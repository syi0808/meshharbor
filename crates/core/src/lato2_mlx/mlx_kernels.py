import itertools
import types
import sys

import mlx.core as mx
import numpy as np
import torch
from torch import nn
from torch.nn import functional as F


def array(tensor, dtype=mx.float16):
    value = tensor.detach().cpu()
    if value.dtype == torch.bfloat16:
        value = value.float()
    return mx.array(value.numpy(), dtype=dtype)


def tensor(value, dtype):
    if value.dtype == mx.bfloat16:
        value = value.astype(mx.float32)
    return torch.from_numpy(np.array(value)).to(dtype=dtype)


def weights(module, permutation=None):
    cached = getattr(module, "_meshharbor_mlx_weights", None)
    if cached is None:
        weight = module.weight if permutation is None else module.weight.permute(*permutation)
        kernel = array(weight.contiguous())
        bias = None if module.bias is None else array(module.bias)
        if bias is None:
            mx.eval(kernel)
        else:
            mx.eval(kernel, bias)
        cached = module._meshharbor_mlx_weights = (kernel, bias)
    return cached


def linear(module, value):
    kernel, bias = weights(module)
    result = array(value) @ kernel.T
    return tensor(result if bias is None else result + bias, value.dtype)


def conv3d(module, value):
    if module.padding_mode != "zeros":
        raise ValueError(f"Unsupported MLX Conv3d padding: {module.padding_mode}")
    kernel, bias = weights(module, (0, 2, 3, 4, 1))
    source = array(value.permute(0, 2, 3, 4, 1).contiguous())
    result = mx.conv3d(source, kernel, module.stride, module.padding, module.dilation, module.groups)
    result = result if bias is None else result + bias
    return tensor(result.transpose(0, 4, 1, 2, 3), value.dtype)


def conv_transpose3d(module, value, output_size=None):
    if output_size is not None or module.groups != 1:
        raise ValueError("Unsupported grouped or custom-size MLX ConvTranspose3d")
    kernel, bias = weights(module, (1, 2, 3, 4, 0))
    source = array(value.permute(0, 2, 3, 4, 1).contiguous())
    result = mx.conv_transpose3d(
        source, kernel, stride=module.stride, padding=module.padding,
        dilation=module.dilation, output_padding=module.output_padding,
    )
    result = result if bias is None else result + bias
    return tensor(result.transpose(0, 4, 1, 2, 3), value.dtype)


def attention(query, key, value, attn_mask=None, dropout_p=0.0, is_causal=False, *, scale=None, enable_gqa=False):
    if dropout_p:
        raise ValueError("LATO.2 MLX only supports inference attention")
    if attn_mask is not None and is_causal:
        raise ValueError("Cannot combine a causal mask and an explicit mask")
    if not enable_gqa and query.shape[-3] != key.shape[-3]:
        raise ValueError("Attention heads differ without GQA")
    mask = "causal" if is_causal else None
    if attn_mask is not None:
        mask = array(attn_mask, mx.bool_ if attn_mask.dtype == torch.bool else mx.float16)
    squeeze = query.ndim == 3
    if squeeze:
        query, key, value = query[None], key[None], value[None]
    result = mx.fast.scaled_dot_product_attention(
        array(query), array(key), array(value), scale=query.shape[-1] ** -0.5 if scale is None else scale, mask=mask,
    )
    return tensor(result[0] if squeeze else result, query.dtype)


def flash(query, key, value, dropout_p=0.0, softmax_scale=None, causal=False):
    return attention(
        query.transpose(1, 2), key.transpose(1, 2), value.transpose(1, 2),
        dropout_p=dropout_p, is_causal=causal, scale=softmax_scale,
        enable_gqa=query.shape[-2] != key.shape[-2],
    ).transpose(1, 2)


def flash_varlen(query, key, value, cu_seqlens_q, cu_seqlens_k, max_seqlen_q, max_seqlen_k,
                 dropout_p=0.0, softmax_scale=None, causal=False):
    queries, keys = cu_seqlens_q.tolist(), cu_seqlens_k.tolist()
    if len(queries) != len(keys):
        raise ValueError("Attention batch offsets differ")
    chunks = []
    for index, (start, end) in enumerate(zip(queries[:-1], queries[1:])):
        key_start, key_end = keys[index:index + 2]
        if end == start or key_end == key_start:
            chunks.append(query.new_zeros((end - start, query.shape[-2], value.shape[-1])))
        else:
            chunks.append(flash(
                query[None, start:end], key[None, key_start:key_end], value[None, key_start:key_end],
                dropout_p, softmax_scale, causal,
            )[0])
    return torch.cat(chunks, dim=0)


def flash_module():
    module = types.ModuleType("flash_attn")
    module.flash_attn_func = flash
    module.flash_attn_varlen_func = flash_varlen
    module.flash_attn_qkvpacked_func = lambda qkv, *args, **kwargs: flash(
        *qkv.unbind(dim=-3), *args, **kwargs,
    )
    module.flash_attn_varlen_qkvpacked_func = lambda qkv, offsets, length, *args, **kwargs: flash_varlen(
        *qkv.unbind(dim=-3), offsets, offsets, length, length, *args, **kwargs,
    )
    module.flash_attn_varlen_kvpacked_func = lambda query, kv, *args, **kwargs: flash_varlen(
        query, *kv.unbind(dim=-3), *args, **kwargs,
    )
    return module


class SparseConvTensor:
    def __init__(self, features, indices, spatial_shape, batch_size, grid=None, voxel_num=None, indice_dict=None):
        self._features = features
        self.indices = indices
        self.spatial_shape = spatial_shape
        self.batch_size = batch_size
        self.grid = grid
        self.voxel_num = voxel_num
        self.indice_dict = {} if indice_dict is None else indice_dict
        self.benchmark = False
        self.benchmark_record = None
        self.thrust_allocator = None
        self._timer = None
        self.force_algo = None
        self.int8_scale = None

    @property
    def features(self):
        return self._features

    @features.setter
    def features(self, value):
        self._features = value

    def replace_feature(self, value):
        return SparseConvTensor(
            value, self.indices, self.spatial_shape, self.batch_size,
            self.grid, self.voxel_num, self.indice_dict,
        )


def neighbours(coordinates, kernel, dilation):
    spatial = coordinates[:, 1:].astype(np.int64)
    radius = (kernel // 2) * dilation
    span = spatial.max(axis=0) + radius * 2 + 1
    stride = np.array([span[1] * span[2], span[2], 1], dtype=np.int64)
    hashes = (spatial + radius) @ stride + coordinates[:, 0] * span.prod()
    order = np.argsort(hashes)
    sorted_hashes = hashes[order]
    if np.any(sorted_hashes[1:] == sorted_hashes[:-1]):
        raise ValueError("Duplicate sites in sparse convolution")
    offsets = np.array(list(itertools.product(range(-radius, radius + 1, dilation), repeat=3)))
    indices = np.empty((len(spatial), len(offsets)), dtype=np.int32)
    for tap, offset in enumerate(offsets @ stride):
        candidates = hashes + offset
        found = np.searchsorted(sorted_hashes, candidates).clip(max=len(spatial) - 1)
        indices[:, tap] = np.where(sorted_hashes[found] == candidates, order[found], len(spatial))
    return indices


class SubMConv3d(nn.Module):
    def __init__(self, in_channels, out_channels, kernel_size, dilation=1, bias=True, indice_key=None, algo=None):
        super().__init__()
        if kernel_size not in (1, 3) or not isinstance(dilation, int) or dilation < 1:
            raise ValueError("LATO.2 MLX supports only 1³ and 3³ submanifold convolution")
        self.out_channels = out_channels
        self.kernel_size = kernel_size
        self.dilation = dilation
        self.weight = nn.Parameter(torch.empty(out_channels, kernel_size, kernel_size, kernel_size, in_channels))
        self.bias = nn.Parameter(torch.empty(out_channels)) if bias else None

    def forward(self, source):
        features = source.features
        kernel, bias = weights(self)
        kernel = kernel.reshape(self.out_channels, -1).T
        packed = array(features)
        if self.kernel_size == 1:
            result = packed @ kernel
            return source.replace_feature(tensor(result if bias is None else result + bias, features.dtype))
        coords = source.indices
        memo = (self.kernel_size, self.dilation)
        cached = source.indice_dict.get(memo)
        if cached is None or cached[0] is not coords:
            if len(coords) > 1_500_000:
                raise ValueError("LATO.2 sparse convolution exceeds its working-set limit")
            cached = (coords, neighbours(coords.cpu().numpy(), self.kernel_size, self.dilation))
            source.indice_dict[memo] = cached
        packed = mx.concatenate((packed, mx.zeros((1, features.shape[1]), dtype=packed.dtype)))
        step = max(1, 8_000_000 // kernel.shape[0])
        output = []
        for start in range(0, len(coords), step):
            indices = cached[1][start:start + step]
            values = mx.take(packed, mx.array(indices), axis=0).reshape(len(indices), -1)
            result = values @ kernel
            output.append(tensor(result if bias is None else result + bias, features.dtype))
        return source.replace_feature(torch.cat(output, dim=0))


class UnsupportedSparseConv(nn.Module):
    def __init__(self, *args, **kwargs):
        super().__init__()
        raise ValueError("Unexpected strided/inverse sparse convolution in the pinned LATO.2 model")


def scatter_mean(value, index, out, dim=-1):
    counts = torch.zeros_like(out)
    out.scatter_add_(dim, index.expand_as(value), value)
    counts.scatter_add_(dim, index.expand_as(value), torch.ones_like(value))
    return out / counts.clamp(min=1)


def install():
    if not mx.metal.is_available():
        raise RuntimeError("LATO.2 MLX requires Apple Silicon with Metal")
    mx.set_default_device(mx.gpu)
    mx.set_cache_limit(128 * 1024 * 1024)
    nn.Linear.forward = linear
    nn.Conv3d.forward = conv3d
    nn.ConvTranspose3d.forward = conv_transpose3d
    F.scaled_dot_product_attention = attention
    flash_ext = flash_module()
    spconv, pytorch = types.ModuleType("spconv"), types.ModuleType("spconv.pytorch")
    spconv.pytorch = pytorch
    pytorch.SparseConvTensor = SparseConvTensor
    pytorch.SubMConv3d = SubMConv3d
    pytorch.SparseConv3d = pytorch.SparseInverseConv3d = UnsupportedSparseConv
    pytorch.ConvAlgo = types.SimpleNamespace(Native="native", MaskImplicitGemm="implicit")
    scatter = types.ModuleType("torch_scatter")
    scatter.scatter_mean = scatter_mean
    sys.modules.update({
        "flash_attn": flash_ext, "spconv": spconv,
        "spconv.pytorch": pytorch, "torch_scatter": scatter,
    })
