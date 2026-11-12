"""MeshHarbor-managed download adapter for the pinned Pixal3D MLX snapshot.

This first-party adapter deliberately leaves the upstream source snapshot
byte-identical. Repository revisions and the NAF checksum are read from the
pinned upstream downloader; files are written only to an explicit app-data
weights directory, and MoGe is omitted for MeshHarbor's fixed-camera profile.
"""

import argparse
import hashlib
import json
from pathlib import Path
import runpy
import shutil
import urllib.request

UPSTREAM_DOWNLOADER_GIT_BLOB = "7e57881be9464b57fe0c79b98d3473a79c6de580"
EXPECTED_PIXAL_CHECKPOINTS = {
    "ss_flow_img_dit_1_3B_64_bf16.safetensors",
    "ss_dec_conv3d_16l8_fp16.safetensors",
    "slat_flow_img2shape_dit_1_3B_512_bf16.safetensors",
    "shape_dec_next_dc_f16c32_fp16.safetensors",
    "slat_flow_img2shape_dit_1_3B_1024_bf16.safetensors",
    "slat_flow_imgshape2tex_dit_1_3B_1024_bf16.safetensors",
    "tex_dec_next_dc_f16c32_fp16.safetensors",
}


def upstream_api(source_root: Path):
    script = source_root / "scripts" / "download_models.py"
    if not script.is_file():
        raise SystemExit(f"Pinned upstream downloader is missing: {script}")
    content = script.read_bytes()
    blob_hash = hashlib.sha1(f"blob {len(content)}\0".encode() + content).hexdigest()
    if blob_hash != UPSTREAM_DOWNLOADER_GIT_BLOB:
        raise SystemExit("The Pixal3D downloader differs from the reviewed pinned source snapshot.")
    return runpy.run_path(str(script), run_name="meshharbor_upstream_downloader")


def check_dino_access(api):
    try:
        api["HfApi"]().model_info(api["DINO_REPO"], revision=api["DINO_REVISION"])
    except Exception:
        # Keep request details private: this may involve the user's gated-model credential.
        raise SystemExit(
            "Cannot access pinned gated DINOv3. Review/request access in Hugging Face, "
            "then use `meshharbor runtime hf-login` with the isolated MeshHarbor cache."
        ) from None
    print("Gated DINOv3 access check passed; no files were downloaded.", flush=True)


def download_models(api, weights: Path):
    weights.mkdir(parents=True, exist_ok=True)
    manifest_path = weights / "download-manifest.json"
    report = {"models": {}, "dino_requested": True, "moge_downloaded": False}

    def record(name, value):
        report["models"][name] = value
        manifest_path.write_text(json.dumps(report, indent=2) + "\n")

    # Fetch the gated repository first. If model-file access differs from the
    # metadata preflight, fail before transferring the much larger Pixal3D set.
    print("Downloading pinned gated DINOv3 files…", flush=True)
    try:
        dino_path = api["snapshot_download"](
            api["DINO_REPO"],
            revision=api["DINO_REVISION"],
            allow_patterns=["*.safetensors", "config.json"],
            max_workers=1,
        )
    except Exception:
        raise SystemExit("DINOv3 download failed. Check gated access and the isolated HF login, then retry.") from None
    record("dinov3", {"repo": api["DINO_REPO"], "revision": api["DINO_REVISION"], "path": dino_path})

    print("Downloading seven pinned Pixal3D checkpoints (~24 GB)…", flush=True)
    try:
        pixal_path = api["snapshot_download"](
            api["PIXAL_REPO"],
            revision=api["PIXAL_REVISION"],
            allow_patterns=["ckpts/*.safetensors", "ckpts/*.json", "pipeline.json"],
            max_workers=2,
        )
    except Exception:
        raise SystemExit("Pixal3D checkpoint download failed; retrying resumes the HF cache.") from None
    record("pixal3d", {"repo": api["PIXAL_REPO"], "revision": api["PIXAL_REVISION"], "path": pixal_path})

    metadata = api["HfApi"]().model_info(api["PIXAL_REPO"], revision=api["PIXAL_REVISION"], files_metadata=True)
    verified = []
    for entry in metadata.siblings:
        if entry.lfs and entry.rfilename.startswith("ckpts/"):
            path = Path(pixal_path) / entry.rfilename
            actual = api["sha256_file"](path)
            if actual != entry.lfs.sha256:
                raise SystemExit(f"Pixal3D checkpoint checksum mismatch: {entry.rfilename}")
            verified.append(entry.rfilename)
    verified_checkpoints = {Path(name).name for name in verified}
    if verified_checkpoints != EXPECTED_PIXAL_CHECKPOINTS:
        raise SystemExit(
            "Pixal3D checkpoint metadata did not verify the complete reviewed seven-file set."
        )
    report["models"]["pixal3d"]["sha256_verified"] = verified
    record("pixal3d", report["models"]["pixal3d"])

    naf_path = weights / "naf_release.pth"
    if naf_path.is_file() and api["sha256_file"](naf_path) != api["NAF_SHA256"]:
        print("Discarding an invalid cached NAF download before retrying…", flush=True)
        naf_path.unlink()
    if not naf_path.is_file():
        temporary = naf_path.with_suffix(".download")
        print("Downloading and verifying the NAF checkpoint…", flush=True)
        try:
            with urllib.request.urlopen(api["NAF_URL"]) as response, temporary.open("wb") as target:
                shutil.copyfileobj(response, target)
            if api["sha256_file"](temporary) != api["NAF_SHA256"]:
                temporary.unlink(missing_ok=True)
                raise SystemExit("NAF checkpoint checksum mismatch; refusing to install the file.")
            temporary.replace(naf_path)
        except Exception:
            temporary.unlink(missing_ok=True)
            raise SystemExit("NAF checkpoint download failed; retrying is safe.") from None
    if api["sha256_file"](naf_path) != api["NAF_SHA256"]:
        raise SystemExit("NAF checkpoint checksum mismatch; refusing to use the file.")
    record("naf", {"url": api["NAF_URL"], "sha256": api["NAF_SHA256"], "path": str(naf_path)})
    print(
        f"Pinned snapshots downloaded; Pixal3D/NAF hashes verified. Manifest: {manifest_path}",
        flush=True,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--weights-dir", type=Path, required=True)
    parser.add_argument("--preflight-only", action="store_true")
    args = parser.parse_args()
    api = upstream_api(args.source_root.resolve())
    if args.preflight_only:
        check_dino_access(api)
        return
    check_dino_access(api)
    download_models(api, args.weights_dir.resolve())


if __name__ == "__main__":
    main()
