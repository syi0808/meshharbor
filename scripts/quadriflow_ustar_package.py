#!/usr/bin/env python3
"""Assemble the installer-shaped QuadriFlow USTAR payload from reviewed files.

This helper only copies five caller-provided payloads into a deterministic,
uncompressed POSIX USTAR archive. It does not build or execute QuadriFlow,
validate signatures/SBOM/license clearance, update a product manifest, or make a
package installable. The release gate remains a separate reviewed action.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import stat
import sys
import tarfile
from pathlib import Path


MAX_ARCHIVE_BYTES = 128 * 1024 * 1024
PLATFORM_EXECUTABLES = {
    "linux-x86_64": "quadriflow",
    "macos-aarch64": "quadriflow",
    "windows-x86_64": "quadriflow.exe",
}
COMMON_PAYLOADS = (
    ("THIRD-PARTY-NOTICES.md", 0o644),
    ("sbom.spdx.json", 0o644),
    ("provenance.json", 0o644),
    ("provenance.sigstore.json", 0o644),
)


def _load_payloads(payload_dir: Path, executable_name: str) -> list[tuple[str, bytes, int]]:
    if payload_dir.is_symlink():
        raise ValueError("payload directory must not be a symlink")
    root = payload_dir.resolve(strict=True)
    if not root.is_dir():
        raise ValueError("payload path must be a directory")

    expected_names = {executable_name, *(name for name, _ in COMMON_PAYLOADS)}
    entries = list(root.iterdir())
    actual_names = {entry.name for entry in entries}
    if actual_names != expected_names:
        missing = sorted(expected_names - actual_names)
        unexpected = sorted(actual_names - expected_names)
        raise ValueError(
            "payload directory members differ from contract "
            f"(missing={missing}, unexpected={unexpected})"
        )

    payloads: list[tuple[str, bytes, int]] = []
    total_bytes = 0
    for name, mode in [(executable_name, 0o755), *COMMON_PAYLOADS]:
        path = root / name
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError(f"payload is not a regular file: {name}")
        if metadata.st_size <= 0:
            raise ValueError(f"payload must not be empty: {name}")
        total_bytes += metadata.st_size
        if total_bytes > MAX_ARCHIVE_BYTES:
            raise ValueError("payload total exceeds the installer's archive bound")
        contents = path.read_bytes()
        if len(contents) != metadata.st_size:
            raise ValueError(f"payload changed while being read: {name}")
        archive_name = f"bin/{name}" if name == executable_name else name
        payloads.append((archive_name, contents, mode))
    return payloads


def build_ustar(payload_dir: Path, output: Path, platform: str) -> dict[str, object]:
    executable_name = PLATFORM_EXECUTABLES.get(platform)
    if executable_name is None:
        raise ValueError("unsupported target platform")

    payloads = _load_payloads(payload_dir, executable_name)
    if output.exists() or output.is_symlink():
        raise ValueError("output path already exists")
    parent = output.parent.resolve(strict=True)
    output_path = parent / output.name
    payload_root = payload_dir.resolve(strict=True)
    if output_path == payload_root or payload_root in output_path.parents:
        raise ValueError("output must be outside the payload directory")

    entries: list[tuple[str, bytes | None, int]] = [("bin/", None, 0o755)]
    entries.extend(payloads)
    try:
        with output_path.open("xb") as stream:
            with tarfile.open(fileobj=stream, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                for name, contents, mode in entries:
                    member = tarfile.TarInfo(name)
                    member.mode = mode
                    member.uid = 0
                    member.gid = 0
                    member.uname = ""
                    member.gname = ""
                    member.mtime = 0
                    if contents is None:
                        member.type = tarfile.DIRTYPE
                        member.size = 0
                        archive.addfile(member)
                    else:
                        member.type = tarfile.REGTYPE
                        member.size = len(contents)
                        archive.addfile(member, io.BytesIO(contents))
            stream.flush()
            os.fsync(stream.fileno())
    except Exception:
        try:
            output_path.unlink()
        except OSError:
            pass
        raise

    size_bytes = output_path.stat().st_size
    if size_bytes > MAX_ARCHIVE_BYTES:
        output_path.unlink()
        raise ValueError("archive exceeds the installer's size bound")

    payload_report = [
        {
            "path": name,
            "size_bytes": len(contents),
            "sha256": hashlib.sha256(contents).hexdigest(),
        }
        for name, contents, _ in payloads
    ]
    digest = hashlib.sha256(output_path.read_bytes()).hexdigest()
    return {
        "format": "tar",
        "platform": platform,
        "size_bytes": size_bytes,
        "sha256": digest,
        "payloads": payload_report,
        "installable": False,
        "release_ready": False,
        "dependency_closure_verified": False,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--payload-dir", required=True, type=Path)
    parser.add_argument("--platform", required=True, choices=sorted(PLATFORM_EXECUTABLES))
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        report = build_ustar(args.payload_dir, args.output, args.platform)
    except (OSError, tarfile.TarError, ValueError) as error:
        print(f"package error: {error}", file=sys.stderr)
        return 2
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
