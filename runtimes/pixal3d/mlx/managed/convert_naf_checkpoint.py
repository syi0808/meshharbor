"""Verify and convert the pinned NAF checkpoint for Pixal3D MLX."""

import argparse
import hashlib
import os
from pathlib import Path

NAF_SHA256 = "c096c1ab2217a5c3ac136365f721685e2201379cb69d509cfb0261183847c98f"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(8 * 1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--weights-dir", type=Path, required=True)
    args = parser.parse_args()
    weights = args.weights_dir.resolve()
    source = weights / "naf_release.pth"
    target = weights / "naf_release.safetensors"
    temporary = weights / ".naf_release.safetensors.tmp"
    if not source.is_file():
        raise SystemExit(f"NAF checkpoint not found: {source}")
    if sha256(source) != NAF_SHA256:
        raise SystemExit("NAF checkpoint checksum mismatch; refusing to load the file.")
    from safetensors import safe_open
    from safetensors.torch import save_file
    if target.is_file() and target.stat().st_size > 0:
        try:
            with safe_open(target, framework="pt", device="cpu") as verified:
                if verified.keys():
                    print(f"Converted NAF weights already exist and are readable: {target}", flush=True)
                    return
        except Exception:
            target.unlink(missing_ok=True)

    import torch

    checkpoint = torch.load(source, map_location="cpu", weights_only=True)
    if not isinstance(checkpoint, dict) or not checkpoint:
        raise SystemExit("NAF checkpoint did not contain a non-empty tensor state dictionary.")
    state = {name: tensor.contiguous() for name, tensor in checkpoint.items()}
    save_file(state, str(temporary))
    with safe_open(temporary, framework="pt", device="cpu") as verified:
        if set(verified.keys()) != set(state):
            temporary.unlink(missing_ok=True)
            raise SystemExit("Converted NAF file failed key verification.")
    os.replace(temporary, target)
    print(f"NAF checkpoint converted and verified: {target}", flush=True)


if __name__ == "__main__":
    main()
