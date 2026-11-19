import os
from pathlib import Path
import sys
from tempfile import NamedTemporaryFile

from huggingface_hub import HfApi, get_hf_file_metadata, get_token, hf_hub_url

HUB = "https://huggingface.co"


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


def is_denied(error):
    return getattr(getattr(error, "response", None), "status_code", None) in (401, 403)


def check_connection(candidate):
    token = candidate or get_token()
    if not token:
        return "not_connected"
    try:
        HfApi(endpoint=HUB).whoami(token=token)
    except Exception as error:
        return "invalid_token" if is_denied(error) else "unavailable"
    if candidate:
        try:
            save_token(candidate)
        except OSError:
            return "storage_error"
    return "connected"


def check_model_access(revision):
    token = get_token()
    if not token:
        return "not_connected"
    try:
        metadata = get_hf_file_metadata(
            hf_hub_url(
                "facebook/dinov3-vitl16-pretrain-lvd1689m",
                "model.safetensors",
                revision=revision,
                endpoint=HUB,
            ),
            token=token,
            timeout=10,
        )
    except Exception as error:
        return "access_denied" if is_denied(error) else "unavailable"
    if metadata.commit_hash != revision or not metadata.size:
        return "unavailable"
    return "verified"


if __name__ == "__main__":
    if sys.argv[1] == "account":
        print(check_connection(sys.stdin.read(513)))
    elif sys.argv[1] == "model":
        print(check_model_access(sys.argv[2]))
