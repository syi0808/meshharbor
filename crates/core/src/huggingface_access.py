import os
from pathlib import Path
import sys
from tempfile import NamedTemporaryFile

from huggingface_hub import get_hf_file_metadata, get_token, hf_hub_url


def save_token(token):
    home = Path(os.environ["HF_HOME"])
    home.mkdir(parents=True, exist_ok=True, mode=0o700)
    temporary = None
    try:
        with NamedTemporaryFile(dir=home, delete=False) as handle:
            temporary = Path(handle.name)
            handle.write(token.encode("ascii"))
        os.replace(temporary, home / "token")
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def check_access(candidate, revision):
    token = candidate or get_token()
    if not token:
        return "not_connected"
    try:
        metadata = get_hf_file_metadata(
            hf_hub_url(
                "facebook/dinov3-vitl16-pretrain-lvd1689m",
                "model.safetensors",
                revision=revision,
                endpoint="https://huggingface.co",
            ),
            token=token,
            timeout=10,
        )
    except Exception as error:
        response = getattr(error, "response", None)
        if getattr(response, "status_code", None) in (401, 403):
            return "access_denied"
        return "unavailable"
    if metadata.commit_hash != revision or not metadata.size:
        return "unavailable"
    if candidate:
        try:
            save_token(candidate)
        except OSError:
            return "storage_error"
    return "verified"


if __name__ == "__main__":
    print(check_access(sys.stdin.read(513), sys.argv[1]))
