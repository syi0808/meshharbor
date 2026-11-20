import argparse
import json
import math
import os
import sys
from pathlib import Path


MAX_PROMPT_BYTES = 4096
MODEL_PATHS = {
    "sparse_structure_flow_model": "model/ckpts/ss_flow_txt_dit_L_16l8_fp16",
    "slat_flow_model": "model/ckpts/slat_flow_txt_dit_L_64l8p2_fp16",
    "sparse_structure_decoder": "decoders/ckpts/ss_dec_conv3d_16l8_fp16",
    "slat_decoder_mesh": "decoders/ckpts/slat_dec_mesh_swin8_B_64l8m256c_fp16",
}


def read_prompt():
    raw = sys.stdin.buffer.read(MAX_PROMPT_BYTES + 1)
    if len(raw) > MAX_PROMPT_BYTES:
        raise ValueError("Text prompt exceeds 4096 UTF-8 bytes.")
    prompt = raw.decode("utf-8")
    if not prompt.strip() or "\0" in prompt:
        raise ValueError("A nonempty text prompt without NUL characters is required.")
    return prompt


def load_pipeline(weights):
    from transformers import AutoTokenizer, CLIPTextModel
    from trellis import models
    from trellis.pipelines import TrellisTextTo3DPipeline, samplers

    class LocalTextPipeline(TrellisTextTo3DPipeline):
        def _init_text_cond_model(self, directory):
            encoder = CLIPTextModel.from_pretrained(
                directory, local_files_only=True, use_safetensors=True
            ).eval().cuda()
            tokenizer = AutoTokenizer.from_pretrained(directory, local_files_only=True)
            self.text_cond_model = {"model": encoder, "tokenizer": tokenizer}
            self.text_cond_model["null_cond"] = self.encode_text([""])

    config = json.loads((weights / "model/pipeline.json").read_text(encoding="utf-8"))
    if config["name"] != "TrellisTextTo3DPipeline":
        raise ValueError("Unexpected TRELLIS text pipeline configuration.")
    settings = config["args"]
    sparse = settings["sparse_structure_sampler"]
    slat = settings["slat_sampler"]
    sampler = samplers.FlowEulerGuidanceIntervalSampler
    if any(spec["name"] != "FlowEulerGuidanceIntervalSampler" for spec in (sparse, slat)):
        raise ValueError("Unexpected TRELLIS text sampler configuration.")

    checkpoints = {}
    for name, relative_path in MODEL_PATHS.items():
        stem = weights / relative_path
        if not stem.with_suffix(".json").is_file() or not stem.with_suffix(".safetensors").is_file():
            raise FileNotFoundError(f"Missing local TRELLIS checkpoint: {relative_path}")
        checkpoints[name] = models.from_pretrained(str(stem))
    pipeline = LocalTextPipeline(
        models=checkpoints,
        sparse_structure_sampler=sampler(**sparse["args"]),
        slat_sampler=sampler(**slat["args"]),
        slat_normalization=settings["slat_normalization"],
        text_cond_model=str(weights / "clip"),
    )
    pipeline.sparse_structure_sampler_params = sparse["params"]
    pipeline.slat_sampler_params = slat["params"]
    pipeline.cuda()
    return pipeline


def stage(name):
    print(f"=== {name} ===", flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("backend", choices=("trellis-text-cuda",))
    parser.add_argument("source", type=Path)
    parser.add_argument("weights", type=Path)
    parser.add_argument("prompt_input", choices=("-",))
    parser.add_argument("output", type=Path)
    parser.add_argument("seed", type=int)
    parser.add_argument("target_faces", type=int)
    args = parser.parse_args()
    prompt = read_prompt()

    os.environ.update(
        ATTN_BACKEND="xformers",
        SPARSE_ATTN_BACKEND="xformers",
        SPARSE_BACKEND="spconv",
        SPCONV_ALGO="native",
    )
    sys.path.insert(0, str(args.source))
    import torch
    import trimesh

    if not torch.cuda.is_available():
        raise RuntimeError("TRELLIS Text Large requires CUDA-enabled PyTorch and an NVIDIA GPU.")
    stage("initialize_model")
    pipeline = load_pipeline(args.weights)
    stage("generate_mesh")
    decoded = pipeline.run(prompt, seed=args.seed, formats=["mesh"])["mesh"][0]
    if not decoded.success:
        raise RuntimeError("TRELLIS did not produce a mesh.")

    stage("mesh_export")
    mesh = trimesh.Trimesh(
        vertices=decoded.vertices.detach().float().cpu().numpy(),
        faces=decoded.faces.detach().cpu().numpy(),
        process=False,
    )
    mesh.apply_transform(trimesh.transformations.rotation_matrix(-math.pi / 2, (1, 0, 0)))
    if len(mesh.faces) > args.target_faces:
        mesh = mesh.simplify_quadric_decimation(face_count=args.target_faces)
    mesh.export(str(args.output))


if __name__ == "__main__":
    main()
