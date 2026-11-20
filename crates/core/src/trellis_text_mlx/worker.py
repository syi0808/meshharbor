import argparse
import gc
import json
import math
import sys
from pathlib import Path


MAX_OCCUPIED_CELLS = 32_768
CHECKPOINTS = {
    "sparse_structure_flow": ("model/ckpts/ss_flow_txt_dit_L_16l8_fp16", "SparseStructureFlowModel"),
    "sparse_structure_decoder": ("decoders/ckpts/ss_dec_conv3d_16l8_fp16", "SparseStructureDecoder"),
    "slat_flow": ("model/ckpts/slat_flow_txt_dit_L_64l8p2_fp16", "SLatFlowModel"),
    "mesh_decoder": ("decoders/ckpts/slat_dec_mesh_swin8_B_64l8m256c_fp16", "SLatMeshDecoder"),
}


def read_prompt():
    raw = sys.stdin.buffer.read(4097)
    if len(raw) > 4096:
        raise ValueError("Text prompt exceeds 4096 UTF-8 bytes.")
    prompt = raw.decode("utf-8")
    if not prompt.strip() or "\0" in prompt:
        raise ValueError("A nonempty text prompt without NUL characters is required.")
    return prompt


def stage(name):
    print(f"=== {name} ===", flush=True)


def json_file(path):
    return json.loads(path.read_text(encoding="utf-8"))


def load_component(weights, component, builder):
    import mlx.core as mx

    relative_path, expected_name = CHECKPOINTS[component]
    base = weights / relative_path
    config = json_file(base.with_suffix(".json"))
    if config["name"] != expected_name:
        raise ValueError(f"Unexpected TRELLIS component: {component}.")
    return builder(config, mx.load(str(base.with_suffix(".safetensors"))))


def load_converted(weights, component, converter):
    def build(config, tensors):
        model = converter.build_model(config)
        model.load_weights(list(converter.convert_weights(tensors, config).items()), strict=True)
        return model.eval()

    return load_component(weights, component, build)


def tokenize_text(path, prompt):
    from tokenizers import Tokenizer

    tokenizer = Tokenizer.from_file(str(path / "tokenizer.json"))
    padding = json_file(path / "tokenizer_config.json")["pad_token"]
    if isinstance(padding, dict):
        padding = padding["content"]
    padding_id = tokenizer.token_to_id(padding)
    if padding_id is None:
        raise ValueError("CLIP padding token is missing from the local tokenizer.")
    tokenizer.enable_truncation(max_length=77)
    tokenizer.enable_padding(length=77, pad_id=padding_id, pad_token=padding)
    return [encoding.ids for encoding in tokenizer.encode_batch([prompt, ""])]


def encode_prompt(weights, prompt):
    import mlx.core as mx
    from mlx_diffuser.converters.sdxl import CLIPTextModelConverter

    path = weights / "clip"
    config = json_file(path / "config.json")["text_config"]
    converter = CLIPTextModelConverter()
    encoder = converter.build_model(config)
    tensors = mx.load(str(path / "model.safetensors"))
    text_params = {key: value for key, value in tensors.items() if key.startswith("text_model.")}
    encoder.load_weights(list(converter.convert_weights(text_params, config).items()), strict=True)
    encoder.eval()
    input_ids = tokenize_text(path, prompt)
    hidden_states, _ = encoder(mx.array(input_ids, dtype=mx.int32))
    result = encoder.text_model.final_layer_norm(hidden_states[-1])
    mx.eval(result)
    return result[:1], result[1:]


def sample_stage(flow, noise, positive, negative, spec):
    from mlx_diffuser.schedulers.trellis_flow import TrellisFlowEulerSampler

    if spec["name"] != "FlowEulerGuidanceIntervalSampler":
        raise ValueError("Unexpected TRELLIS text sampler.")
    sampler = TrellisFlowEulerSampler(**spec["args"])
    return sampler.sample(flow, noise, positive, negative_cond=negative, **spec["params"]).samples


def sparse_structure(weights, positive, negative, spec, key):
    import mlx.core as mx
    from mlx_diffuser.converters.trellis import (
        TrellisSparseStructureDecoderConverter,
        TrellisSparseStructureFlowConverter,
    )

    flow = load_converted(weights, "sparse_structure_flow", TrellisSparseStructureFlowConverter())
    noise = mx.random.normal((1, *(flow.config.resolution,) * 3, flow.config.in_channels), key=key)
    latent = sample_stage(flow, noise, positive, negative, spec)
    mx.eval(latent)
    del flow
    gc.collect()
    mx.clear_cache()
    decoder = load_converted(weights, "sparse_structure_decoder", TrellisSparseStructureDecoderConverter())
    coords = decoder.occupied_coordinates(latent)
    mx.eval(coords)
    if not 0 < coords.shape[0] <= MAX_OCCUPIED_CELLS:
        raise RuntimeError(f"TRELLIS produced {coords.shape[0]} occupied cells; try another seed or prompt.")
    return coords


def structured_latent(weights, coords, positive, negative, settings, key):
    import mlx.core as mx
    from mlx_diffuser.converters.trellis import TrellisSLatFlowConverter
    from mlx_diffuser.layers.sparse import SparseTensor

    flow = load_converted(weights, "slat_flow", TrellisSLatFlowConverter())
    noise = SparseTensor(
        mx.random.normal((coords.shape[0], flow.config.in_channels), key=key),
        coords, batch_size=1, spatial_shape=(flow.config.resolution,) * 3,
    )
    latent = sample_stage(
        lambda sample, time, cond: flow(sample, time, cond, low_memory=True),
        noise, positive, negative, settings["slat_sampler"],
    )
    normalization = settings["slat_normalization"]
    features = latent.features * mx.array(normalization["std"]) + mx.array(normalization["mean"])
    mx.eval(features)
    return SparseTensor(features, coords, batch_size=1, spatial_shape=noise.spatial_shape)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("backend", choices=("trellis-text-mlx",))
    parser.add_argument("source", type=Path)
    parser.add_argument("weights", type=Path)
    parser.add_argument("prompt_input", choices=("-",))
    parser.add_argument("output", type=Path)
    parser.add_argument("seed", type=int)
    parser.add_argument("target_faces", type=int)
    args = parser.parse_args()
    prompt = read_prompt()

    sys.path.insert(0, str(args.source / "src"))
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import mlx.core as mx
    import numpy as np
    import trimesh

    if not mx.metal.is_available():
        raise RuntimeError("TRELLIS Text MLX requires Apple Silicon with Metal.")
    mx.set_default_device(mx.gpu)
    settings = json_file(args.weights / "model/pipeline.json")
    if settings["name"] != "TrellisTextTo3DPipeline":
        raise ValueError("Unexpected TRELLIS text pipeline configuration.")
    settings = settings["args"]
    sparse_key, slat_key = mx.random.split(mx.random.key(args.seed))
    stage("text_conditioning")
    positive, negative = encode_prompt(args.weights, prompt)
    gc.collect()
    mx.clear_cache()
    stage("generate_structure")
    coords = sparse_structure(args.weights, positive, negative, settings["sparse_structure_sampler"], sparse_key)
    gc.collect()
    mx.clear_cache()
    stage("generate_latents")
    latent = structured_latent(args.weights, coords, positive, negative, settings, slat_key)
    gc.collect()
    mx.clear_cache()
    stage("generate_mesh")
    from mesh_decoder import build_mesh_decoder
    from surface import extract_surface, load_tables

    decoder = load_component(args.weights, "mesh_decoder", build_mesh_decoder)
    decoded = decoder(latent)
    features = np.array(decoded.features, dtype=np.float32)
    coords = np.array(decoded.coords[:, 1:])
    resolution = decoded.spatial_shape[0]
    del decoder, decoded, latent
    gc.collect()
    mx.clear_cache()

    stage("mesh_export")
    tables = load_tables(args.source / "flexicubes/tables.py")
    vertices, faces = extract_surface(coords, features, resolution, tables)
    mesh = trimesh.Trimesh(vertices=vertices, faces=faces, process=False)
    mesh.apply_transform(trimesh.transformations.rotation_matrix(-math.pi / 2, (1, 0, 0)))
    if len(mesh.faces) > args.target_faces:
        mesh = mesh.simplify_quadric_decimation(face_count=args.target_faces)
    mesh.export(str(args.output))


if __name__ == "__main__":
    main()
