import os
from pathlib import Path
import sys

import torch
import transformers.modeling_utils

sys.path.insert(0, str(Path(__file__).resolve().parent))
import mlx_kernels
import mesh_conditioning


def no_download(*args, **kwargs):
    raise RuntimeError("LATO.2 model files must be installed locally before inference")


def load_model(cls, checkpoint, device):
    saved = torch.load(checkpoint, map_location="cpu", weights_only=True, mmap=True)
    model = cls(**saved["config"]["args"])
    model.load_state_dict(saved["state_dict"], strict=True, assign=True)
    print(f"LATO.2 MLX loaded {Path(checkpoint).name}", flush=True)
    return model.to(device).eval(), saved["config"]


def main():
    script = Path(sys.argv[1]).resolve()
    code = script.read_text()
    if script.name not in {"e2e_inference.py", "image_e2e_inference.py"}:
        raise RuntimeError("Unexpected upstream LATO.2 entry point")
    cuda_device = 'torch.device("cuda")'
    cuda_autocast = 'torch.autocast("cuda", dtype=torch.bfloat16)'
    if code.count(cuda_device) != 1 or code.count(cuda_autocast) != 2:
        raise RuntimeError("The upstream script does not match the supported LATO.2 revision")
    code = code.replace(cuda_device, 'torch.device("cpu")').replace(cuda_autocast, 'torch.autocast("cpu", enabled=False)')
    os.environ.update({"SPARSE_BACKEND": "spconv", "SPARSE_ATTN_BACKEND": "flash_attn"})
    torch.set_num_threads(4)
    torch.hub.download_url_to_file = no_download
    mlx_kernels.install()
    mesh_conditioning.install()
    sys.path.insert(0, str(script.parent.parent))
    import models
    from utils import load
    sys.modules.pop("flash_attn", None)
    load.load_latov2_model = load_model
    sys.argv = sys.argv[1:]
    print("LATO.2 MLX Metal worker (CPU preprocessing and host graph)", flush=True)
    exec(compile(code, str(script), "exec"), {"__name__": "__main__", "__file__": str(script)})


if __name__ == "__main__":
    main()
