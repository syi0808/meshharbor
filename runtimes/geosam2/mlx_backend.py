"""MLX neural modules for the pinned, upstream GeoSAM2 predictor."""

import re
from types import MethodType

import mlx.core as mx
import mlx.nn as nn
from mlx.utils import tree_flatten
import numpy as np
import torch
from src.sam2.build import MODEL_CONFIGS
from src.sam2.hiera import Hiera
from src.sam2.image_encoder import FpnNeck, ImageEncoder
from src.sam2.mask_decoder import MaskDecoder, PromptEncoder
from src.sam2.memory import MemoryAttention, MemoryEncoder, RoPEAttention
from src.sam2.position_encoding import PositionEmbeddingSine, apply_rotary_enc, compute_axial_cis
from src.sam2.sam2_base import SAM2Base


def encoder():
    config = MODEL_CONFIGS["base_plus"]
    trunk = Hiera(**{key: val for key, val in config.items() if key != "backbone_channel_list"})
    neck = FpnNeck(
        PositionEmbeddingSine(256, normalize=True), 256, config["backbone_channel_list"],
        fpn_top_down_levels=[2, 3], fpn_interp_model="nearest",
    )
    return ImageEncoder(trunk, neck, scalp=1)


def conv_channel_first(layer, values):
    return layer(values.transpose(0, 2, 3, 1)).transpose(0, 3, 1, 2)


class FeatureFusion(nn.Module):
    def __init__(self):
        super().__init__()
        self.fusion_high = nn.Conv2d(512, 256, kernel_size=3, padding=1)
        self.fusion_mid = nn.Conv2d(512, 256, kernel_size=3, padding=1)
        self.fusion_low = nn.Conv2d(512, 256, kernel_size=3, padding=1)

    def __call__(self, normals, positions):
        layers = (self.fusion_high, self.fusion_mid, self.fusion_low)
        return [
            normal + conv_channel_first(layer, mx.concatenate((normal, position), axis=1))
            for layer, normal, position in zip(layers, normals, positions, strict=True)
        ]


class BoundedCrossAttention(RoPEAttention):
    def __call__(self, queries, keys, values, frequencies, num_k_exclude_rope=0):
        def project(layer, sequence):
            length, batch, _ = sequence.shape
            return layer(sequence).reshape(length, batch, self.num_heads, self.head_dim).transpose(1, 2, 0, 3)

        query = project(self.q_proj, queries)
        key = project(self.k_proj, keys)
        value = project(self.v_proj, values)
        spatial = keys.shape[0] - num_k_exclude_rope
        if spatial:
            query, rotated = apply_rotary_enc(query, key[:, :, :spatial], frequencies, repeat_freqs_k=True)
            key = mx.concatenate((rotated, key[:, :, spatial:]), axis=2) if num_k_exclude_rope else rotated
        mx.eval(query, key, value)
        limit = 512 if keys.shape[0] > 8192 else queries.shape[0]
        pieces = []
        for start in range(0, queries.shape[0], limit):
            piece = mx.fast.scaled_dot_product_attention(query[:, :, start:start + limit], key, value, scale=self.scale)
            mx.eval(piece)
            pieces.append(piece)
        length, batch, _ = queries.shape
        output = mx.concatenate(pieces, axis=2).transpose(2, 0, 1, 3).reshape(length, batch, -1)
        return self.out_proj(output)


class Network(SAM2Base):
    def __init__(self):
        memory_attention = MemoryAttention()
        for layer in memory_attention.layers:
            layer.cross_attn = BoundedCrossAttention(d_model=256, num_heads=1, kv_in_dim=64, rope_k_repeat=True)
        super().__init__(
            encoder(), MaskDecoder(), PromptEncoder(), memory_attention, MemoryEncoder(),
            no_obj_embed_spatial=True, add_tpos_enc_to_obj_ptrs=True,
            proj_tpos_enc_in_obj_ptrs=True, use_signed_tpos_enc_to_obj_ptrs=True,
        )
        self.pos_map_encoder = encoder()
        self.feature_fusion = FeatureFusion()

    def geometry_features(self, positions, normals):
        normal_features = self.image_encoder(normals)
        position_features = self.pos_map_encoder(positions)
        fpn = self.feature_fusion(normal_features["backbone_fpn"], position_features["backbone_fpn"])
        return {
            "vision_features": fpn[-1],
            "vision_pos_enc": normal_features["vision_pos_enc"],
            "backbone_fpn": [
                conv_channel_first(self.mask_decoder.conv_s0, fpn[0]),
                conv_channel_first(self.mask_decoder.conv_s1, fpn[1]),
                fpn[2],
            ],
        }


def weight_key(key):
    for torch_name, mlx_name in (
        ("sam_mask_decoder.", "mask_decoder."),
        ("sam_prompt_encoder.", "prompt_encoder."),
    ):
        if key.startswith(torch_name):
            key = mlx_name + key[len(torch_name):]
            break
    key = key.replace(".qkv.qkv.", ".qkv.").replace(".patch_embed.proj.", ".patch_embed.")
    return key.replace(".conv.", ".") if ".neck.convs." in key else key


def weight_layout(key):
    key = weight_key(key)
    for torch_name, mlx_name in (
        (".final_attn_token_to_image.", ".final_attn."),
        (".norm_final_attn.", ".norm_final."),
        (".cross_attn_image.", ".cross_attn."),
        (".cross_attn_to_token.", ".cross_attn_image_to_token."),
        (".mask_downscaling.", ".mask_input."),
        (".pwconv1.", ".pw1."),
        (".pwconv2.", ".pw2."),
        (".positional_encoding_gaussian_matrix", ".gaussian_matrix"),
        (".mask_downsampler.encoder.", ".mask_downsampler.layers."),
        (".mask_downsampler.layers.12.", ".mask_downsampler.final_conv."),
    ):
        key = key.replace(torch_name, mlx_name)
    return re.sub(r"\.(output_upscaling|mask_input)\.(\d+)\.", r".\1.layers.\2.", key)


def patch_lora(key, weight, checkpoint):
    prefix = key.removesuffix("qkv.weight")
    for branch, end in (("q", False), ("v", True)):
        left = checkpoint[prefix + f"linear_b_{branch}.weight"].numpy()
        right = checkpoint[prefix + f"linear_a_{branch}.weight"].numpy()
        delta = left @ right
        if end:
            weight[-delta.shape[0]:] += delta
        else:
            weight[:delta.shape[0]] += delta
    return weight


def load_network(checkpoint):
    network = Network()
    expected = dict(tree_flatten(network.parameters()))
    converted = {}
    for key, tensor in checkpoint.items():
        if re.search(r"\.attn\.qkv\.linear_[ab]_[qv]\.weight$", key):
            continue
        target = weight_layout(key)
        weight = tensor.detach().float().cpu().numpy()
        if key.endswith(".attn.qkv.qkv.weight"):
            weight = patch_lora(key, weight.copy(), checkpoint)
        if weight.ndim == 4 and target.endswith(".weight"):
            if target in ("mask_decoder.output_upscaling.layers.0.weight", "mask_decoder.output_upscaling.layers.3.weight"):
                weight = weight.transpose(1, 2, 3, 0)
            else:
                weight = weight.transpose(0, 2, 3, 1)
        if target in converted:
            raise ValueError(f"Duplicate MLX checkpoint key: {target}")
        converted[target] = mx.array(weight)
    missing = expected.keys() - converted.keys()
    unexpected = converted.keys() - expected.keys()
    wrong_shapes = {
        key: (expected[key].shape, converted[key].shape)
        for key in expected.keys() & converted.keys()
        if expected[key].shape != converted[key].shape
    }
    if missing or unexpected or wrong_shapes:
        raise ValueError(
            f"GeoSAM2 checkpoint mismatch: missing={sorted(missing)}, "
            f"unexpected={sorted(unexpected)}, incompatible_shapes={wrong_shapes}"
        )
    network.load_weights(list(converted.items()), strict=True)
    network.eval()
    mx.eval(network.parameters())
    return network


def to_mlx(value):
    return mx.array(value.detach().cpu().numpy())


def to_torch(value):
    return torch.from_numpy(np.array(value, copy=True))


class Bridge(torch.nn.Module):
    def __init__(self, invoke):
        super().__init__()
        self.invoke = invoke

    def forward(self, *args, **kwargs):
        return self.invoke(*args, **kwargs)


class PromptBridge(Bridge):
    def __init__(self, native):
        super().__init__(native)
        self.mask_input_size = (native.image_embedding_size * 4,) * 2
        self.dense_pe = native.pe_layer((native.image_embedding_size,) * 2)[None]

    def get_dense_pe(self):
        return to_torch(self.dense_pe)

    def forward(self, *, points, boxes, masks):
        coords, labels = points if points is not None else (None, None)
        sparse, dense = self.invoke(
            None if coords is None else to_mlx(coords), None if labels is None else to_mlx(labels),
            boxes=None if boxes is None else to_mlx(boxes), masks=None if masks is None else to_mlx(masks),
        )
        return to_torch(sparse), to_torch(dense)


def use_mlx(predictor, checkpoint):
    """Keep upstream propagation and fusion logic, with model layers running on MLX."""
    mx.set_default_device(mx.gpu)
    mx.set_cache_limit(256 * 1024 * 1024)
    native = load_network(checkpoint)
    frequencies = compute_axial_cis(256, 64, 64)

    def features(_predictor, image, position, normal):
        result = native.geometry_features(to_mlx(position), to_mlx(normal))
        return {
            "vision_features": to_torch(result["vision_features"]),
            "vision_pos_enc": [to_torch(value) for value in result["vision_pos_enc"]],
            "backbone_fpn": [to_torch(value) for value in result["backbone_fpn"]],
        }

    def decode(*, image_embeddings, image_pe, sparse_prompt_embeddings, dense_prompt_embeddings,
               multimask_output, repeat_image, high_res_features):
        if repeat_image:
            raise ValueError("GeoSAM2 predictor must batch its image before decoding.")
        masks, scores, tokens, object_scores = native.mask_decoder(
            to_mlx(image_embeddings), to_mlx(image_pe),
            to_mlx(sparse_prompt_embeddings), to_mlx(dense_prompt_embeddings),
            multimask_output=multimask_output,
            high_res_features=[to_mlx(value) for value in high_res_features],
        )
        return to_torch(masks), to_torch(mx.sigmoid(scores)), to_torch(tokens), to_torch(object_scores)

    def remember(features, masks, *, skip_mask_sigmoid):
        features, positions = native.memory_encoder(
            to_mlx(features), to_mlx(masks), skip_mask_sigmoid=skip_mask_sigmoid,
        )
        return {"vision_features": to_torch(features), "vision_pos_enc": [to_torch(positions)]}

    def attend(*, curr, curr_pos, memory, memory_pos, num_obj_ptr_tokens):
        result = native.memory_attention(
            to_mlx(curr[0]), to_mlx(memory), to_mlx(curr_pos[0]), to_mlx(memory_pos),
            frequencies, num_obj_ptr_tokens,
        )
        return to_torch(result)

    predictor.forward_image = MethodType(features, predictor)
    predictor.image_encoder = None
    predictor.pos_map_encoder = None
    predictor.feature_fusion = None
    predictor.sam_prompt_encoder = PromptBridge(native.prompt_encoder)
    predictor.sam_mask_decoder = Bridge(decode)
    predictor.memory_encoder = Bridge(remember)
    predictor.memory_attention = Bridge(attend)
    predictor.obj_ptr_proj = Bridge(lambda values: to_torch(native.obj_ptr_proj(to_mlx(values))))
    predictor.obj_ptr_tpos_proj = Bridge(lambda values: to_torch(native.obj_ptr_tpos_proj(to_mlx(values))))
    predictor.mask_downsample = Bridge(lambda values: to_torch(conv_channel_first(native.mask_downsample, to_mlx(values))))
    return predictor.eval()
