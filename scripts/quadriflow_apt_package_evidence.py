#!/usr/bin/env python3
"""Capture an offline, explicit-scope snapshot of installed Debian package identity and notices.

This tool does not install packages, access the network, infer dependency closure, or grant
release/license clearance. The caller must pass every binary package whose evidence is wanted.
The output always marks dependency closure and repository origin as unverified. It assumes the
package database and documentation tree are quiescent while being read; it is not a sandbox against
concurrent hostile filesystem mutation.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from typing import Callable

SCHEMA_VERSION = 1
MAX_PACKAGES = 256
MAX_NOTICE_BYTES = 2 * 1024 * 1024
MAX_TOTAL_NOTICE_BYTES = 16 * 1024 * 1024
MAX_METADATA_BYTES = 1024 * 1024
LIMITATIONS = [
    "The caller-supplied package list is not verified as a complete dependency closure.",
    "Repository origin and downloaded .deb/source archive checksums are not captured.",
    (
        "Copyright notice hashes are inventory evidence, not license clearance or "
        "redistribution approval."
    ),
]
PACKAGE = re.compile(r"^[a-z0-9][a-z0-9.+-]{0,127}(?::[a-z0-9][a-z0-9-]{0,31})?$")
VERSION = re.compile(r"^[A-Za-z0-9.+:~-]{1,256}$")
ARCH = re.compile(r"^[a-z0-9][a-z0-9-]{0,31}$")
DPKG_QUERY_FORMAT = (
    "${binary:Package}\t${Version}\t${Architecture}\t"
    "${source:Package}\t${source:Version}\t${db:Status-Status}\n"
)


class EvidenceError(ValueError):
    """Raised when installed-package evidence is missing, unsafe, or ambiguous."""


def _text(value: str, field: str, pattern: re.Pattern[str]) -> str:
    if not isinstance(value, str) or not pattern.fullmatch(value):
        raise EvidenceError(f"invalid or missing {field}")
    return value


def _run_query(command: list[str], runner: Callable[..., subprocess.CompletedProcess[str]]) -> str:
    try:
        completed = runner(
            command,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except OSError as error:
        raise EvidenceError(f"could not run {Path(command[0]).name}") from error
    if completed.returncode != 0:
        raise EvidenceError(
            f"{Path(command[0]).name} failed ({completed.returncode}): "
            f"{completed.stderr.strip()[:500]}"
        )
    return completed.stdout


def _parse_package_row(output: str, requested: str) -> dict[str, str]:
    rows = output.splitlines()
    if len(rows) != 1:
        raise EvidenceError(f"dpkg-query did not return exactly one row for {requested}")
    fields = rows[0].split("\t")
    if len(fields) != 6:
        raise EvidenceError(f"dpkg-query returned an unexpected field count for {requested}")
    binary_package = _text(fields[0], "binary package", PACKAGE)
    binary_name, _, binary_arch = binary_package.partition(":")
    requested_name, separator, requested_arch = requested.partition(":")
    if binary_name != requested_name:
        raise EvidenceError(f"dpkg-query returned a different package than requested: {requested}")
    version = _text(fields[1], "binary version", VERSION)
    architecture = _text(fields[2], "architecture", ARCH)
    if separator and requested_arch != architecture:
        raise EvidenceError(f"requested architecture does not match installed package: {requested}")
    if binary_arch and binary_arch != architecture:
        raise EvidenceError(f"binary package architecture does not match its metadata: {requested}")
    source_package = _text(fields[3], "source package", PACKAGE)
    source_version = _text(fields[4], "source version", VERSION)
    if fields[5] != "installed":
        raise EvidenceError(f"package is not fully installed: {requested}")
    return {
        "binary_package": binary_package,
        "binary_version": version,
        "architecture": architecture,
        "package_status": "installed",
        "source_package": source_package,
        "source_version": source_version,
    }


def _notice_record(
    package: dict[str, str],
    documentation_root: Path,
    runner: Callable[..., subprocess.CompletedProcess[str]],
) -> tuple[dict[str, object], bytes]:
    root = documentation_root.resolve(strict=True)
    if not root.is_dir():
        raise EvidenceError("documentation root is not a directory")
    package_name = package["binary_package"].split(":", 1)[0]
    requested_path = root / package_name / "copyright"
    try:
        resolved = requested_path.resolve(strict=True)
    except OSError as error:
        raise EvidenceError(
            f"copyright notice is missing for {package['binary_package']}"
        ) from error
    try:
        relative_resolved = resolved.relative_to(root)
    except ValueError as error:
        raise EvidenceError(
            f"copyright notice escapes the documentation root for {package['binary_package']}"
        ) from error
    try:
        mode = resolved.stat().st_mode
        if not stat.S_ISREG(mode):
            raise EvidenceError(
                f"copyright notice is not a regular file for {package['binary_package']}"
            )
        if resolved.stat().st_size > MAX_NOTICE_BYTES:
            raise EvidenceError(
                f"copyright notice exceeds size limit for {package['binary_package']}"
            )
        contents = resolved.read_bytes()
    except OSError as error:
        raise EvidenceError(
            f"could not read copyright notice for {package['binary_package']}"
        ) from error
    if not contents or len(contents) > MAX_NOTICE_BYTES or b"\0" in contents:
        raise EvidenceError(
            f"copyright notice is empty, binary, or oversized for {package['binary_package']}"
        )
    try:
        contents.decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise EvidenceError(
            f"copyright notice is not UTF-8 for {package['binary_package']}"
        ) from error

    try:
        owner_output = _run_query(["dpkg-query", "-S", str(resolved)], runner)
    except EvidenceError as error:
        raise EvidenceError(
            f"dpkg-query could not identify the copyright owner for {package['binary_package']}"
        ) from error
    owners = []
    queried_path = str(resolved)
    for line in owner_output.splitlines():
        if ": " not in line:
            raise EvidenceError("dpkg-query returned an unexpected copyright owner row")
        owner_field, reported_path = line.rsplit(": ", 1)
        if reported_path != queried_path:
            raise EvidenceError("dpkg-query returned a different copyright path")
        for owner in (part.strip() for part in owner_field.split(",")):
            owners.append(_text(owner, "copyright owner", PACKAGE))
    if not owners:
        raise EvidenceError(
            f"dpkg-query did not identify the copyright owner for {package['binary_package']}"
        )

    evidence_path = f"notices/{package['binary_package'].replace(':', '_')}/copyright"
    record: dict[str, object] = {
        **package,
        "copyright_notice": {
            "requested_path": f"{package_name}/copyright",
            "resolved_path": relative_resolved.as_posix(),
            "evidence_path": evidence_path,
            "owner_packages": sorted(set(owners)),
            "bytes": len(contents),
            "sha256": hashlib.sha256(contents).hexdigest(),
        },
    }
    return record, contents


def _strict_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise EvidenceError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def validate_evidence_structure(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != {
        "schema_version",
        "evidence_kind",
        "capture_status",
        "scope",
        "dependency_closure_verified",
        "repository_origin_verified",
        "release_ready",
        "limitations",
        "packages",
    }:
        raise EvidenceError("evidence object has missing or unknown fields")
    if (
        isinstance(value["schema_version"], bool)
        or value["schema_version"] != SCHEMA_VERSION
        or value["evidence_kind"] != "apt-dpkg-package-identity-and-copyright-snapshot"
        or value["capture_status"] != "complete_for_explicit_packages"
        or value["scope"] != "explicit-package-list-only"
        or value["dependency_closure_verified"] is not False
        or value["repository_origin_verified"] is not False
        or value["release_ready"] is not False
        or value["limitations"] != LIMITATIONS
    ):
        raise EvidenceError("evidence status or limitations do not match the fail-closed contract")
    packages = value["packages"]
    if not isinstance(packages, list) or not packages or len(packages) > MAX_PACKAGES:
        raise EvidenceError("evidence package list is empty or exceeds its limit")
    seen: set[str] = set()
    total_bytes = 0
    previous = ""
    for package in packages:
        if not isinstance(package, dict) or set(package) != {
            "binary_package",
            "binary_version",
            "architecture",
            "package_status",
            "source_package",
            "source_version",
            "copyright_notice",
        }:
            raise EvidenceError("package evidence has missing or unknown fields")
        name = _text(package["binary_package"], "binary package", PACKAGE)
        if name in seen or name < previous:
            raise EvidenceError("package evidence must be uniquely sorted by binary package")
        previous = name
        seen.add(name)
        _text(package["binary_version"], "binary version", VERSION)
        _text(package["architecture"], "architecture", ARCH)
        if package["package_status"] != "installed":
            raise EvidenceError("package status is not installed")
        binary_arch = name.partition(":")[2]
        if binary_arch and binary_arch != package["architecture"]:
            raise EvidenceError("binary package architecture does not match evidence")
        _text(package["source_package"], "source package", PACKAGE)
        _text(package["source_version"], "source version", VERSION)
        notice = package["copyright_notice"]
        if not isinstance(notice, dict) or set(notice) != {
            "requested_path",
            "resolved_path",
            "evidence_path",
            "owner_packages",
            "bytes",
            "sha256",
        }:
            raise EvidenceError("copyright evidence has missing or unknown fields")
        package_name = name.split(":", 1)[0]
        if notice["requested_path"] != f"{package_name}/copyright":
            raise EvidenceError("copyright requested path does not match package")
        resolved_path = notice["resolved_path"]
        if (
            not isinstance(resolved_path, str)
            or "\\" in resolved_path
            or resolved_path.startswith("/")
            or any(part in {"", ".", ".."} for part in resolved_path.split("/"))
            or not resolved_path.endswith("/copyright")
            or any(
                not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+ -]{0,255}", part)
                for part in resolved_path.split("/")
            )
        ):
            raise EvidenceError("copyright resolved path is unsafe")
        expected_evidence_path = f"notices/{name.replace(':', '_')}/copyright"
        if notice["evidence_path"] != expected_evidence_path:
            raise EvidenceError("copyright evidence path does not match package")
        owners = notice["owner_packages"]
        if not isinstance(owners, list) or not owners or len(owners) > 16:
            raise EvidenceError("copyright owners are missing or exceed their limit")
        normalized_owners = [_text(owner, "copyright owner", PACKAGE) for owner in owners]
        if normalized_owners != sorted(set(normalized_owners)):
            raise EvidenceError("copyright owner list must be unique and sorted")
        byte_count = notice["bytes"]
        if (
            isinstance(byte_count, bool)
            or not isinstance(byte_count, int)
            or not 1 <= byte_count <= MAX_NOTICE_BYTES
        ):
            raise EvidenceError("copyright byte count is invalid")
        total_bytes += byte_count
        if total_bytes > MAX_TOTAL_NOTICE_BYTES:
            raise EvidenceError("copyright evidence exceeds total size limit")
        if not isinstance(notice["sha256"], str) or not re.fullmatch(
            r"[0-9a-f]{64}", notice["sha256"]
        ):
            raise EvidenceError("copyright SHA-256 is invalid")
    return value


def verify_bundle(bundle_value: str | Path) -> dict[str, object]:
    bundle = Path(bundle_value)
    if bundle.is_symlink() or not bundle.is_dir():
        raise EvidenceError("evidence bundle must be a real directory")
    manifest_path = bundle / "evidence.json"
    if (
        manifest_path.is_symlink()
        or not manifest_path.is_file()
        or manifest_path.stat().st_size > MAX_METADATA_BYTES
    ):
        raise EvidenceError("evidence.json is missing, unsafe, or oversized")
    try:
        raw = manifest_path.read_bytes()
        evidence = json.loads(raw.decode("utf-8"), object_pairs_hook=_strict_object)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise EvidenceError("evidence.json is not readable strict UTF-8 JSON") from error
    validate_evidence_structure(evidence)

    expected_files = {"evidence.json"}
    total_bytes = 0
    for package in evidence["packages"]:
        notice = package["copyright_notice"]
        relative = notice["evidence_path"]
        expected_files.add(relative)
        path = bundle / relative
        current = bundle
        for part in Path(relative).parts:
            current = current / part
            if current.is_symlink():
                raise EvidenceError("evidence bundle contains a symlink")
        if not path.is_file() or path.stat().st_size != notice["bytes"]:
            raise EvidenceError("copied copyright notice is missing or has a different byte count")
        contents = path.read_bytes()
        if not contents or b"\0" in contents:
            raise EvidenceError("copied copyright notice is empty or binary")
        try:
            contents.decode("utf-8", errors="strict")
        except UnicodeDecodeError as error:
            raise EvidenceError("copied copyright notice is not UTF-8") from error
        if hashlib.sha256(contents).hexdigest() != notice["sha256"]:
            raise EvidenceError("copied copyright notice hash mismatch")
        total_bytes += len(contents)
    actual_files: set[str] = set()

    def walk_error(error: OSError) -> None:
        raise EvidenceError("evidence bundle could not be scanned completely") from error

    for directory, dirs, files in os.walk(bundle, followlinks=False, onerror=walk_error):
        base = Path(directory)
        if any((base / name).is_symlink() for name in dirs + files):
            raise EvidenceError("evidence bundle contains a symlink")
        for name in files:
            path = base / name
            if not stat.S_ISREG(path.stat().st_mode):
                raise EvidenceError("evidence bundle contains a non-regular file")
            actual_files.add(path.relative_to(bundle).as_posix())
    if actual_files != expected_files:
        raise EvidenceError("evidence bundle has missing or unexpected files")
    if total_bytes > MAX_TOTAL_NOTICE_BYTES:
        raise EvidenceError("evidence bundle exceeds total notice size limit")
    return evidence


def collect(
    packages: list[str],
    output_value: str | Path,
    *,
    documentation_root: str | Path = "/usr/share/doc",
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> Path:
    if not packages or len(packages) > MAX_PACKAGES:
        raise EvidenceError("a nonempty bounded explicit package list is required")
    if any(not isinstance(package, str) or not PACKAGE.fullmatch(package) for package in packages):
        raise EvidenceError("package names must be safe dpkg binary package identifiers")
    if len(set(packages)) != len(packages):
        raise EvidenceError("duplicate package names are not allowed")

    records: list[dict[str, object]] = []
    notices: list[tuple[str, bytes]] = []
    total_notice_bytes = 0
    for requested in sorted(packages):
        query = _run_query(
            [
                "dpkg-query",
                "--show",
                f"--showformat={DPKG_QUERY_FORMAT}",
                requested,
            ],
            runner,
        )
        package_record = _parse_package_row(query, requested)
        record, contents = _notice_record(package_record, Path(documentation_root), runner)
        total_notice_bytes += len(contents)
        if total_notice_bytes > MAX_TOTAL_NOTICE_BYTES:
            raise EvidenceError("copyright evidence exceeds total size limit")
        records.append(record)
        notices.append((str(record["copyright_notice"]["evidence_path"]), contents))
    records.sort(key=lambda item: str(item["binary_package"]))

    evidence = {
        "schema_version": SCHEMA_VERSION,
        "evidence_kind": "apt-dpkg-package-identity-and-copyright-snapshot",
        "capture_status": "complete_for_explicit_packages",
        "scope": "explicit-package-list-only",
        "dependency_closure_verified": False,
        "repository_origin_verified": False,
        "release_ready": False,
        "limitations": LIMITATIONS,
        "packages": records,
    }

    output = Path(output_value)
    if not output.is_absolute():
        raise EvidenceError("output path must be absolute")
    parent = output.parent.resolve(strict=True)
    output = parent / output.name
    if output.exists() or output.is_symlink():
        raise EvidenceError("refusing to overwrite existing evidence output")
    if not output.name or output.name in {".", ".."}:
        raise EvidenceError("invalid evidence output name")

    stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.stage-", dir=parent))
    try:
        for relative, contents in notices:
            target = stage / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(contents)
        (stage / "evidence.json").write_text(
            json.dumps(evidence, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )
        verify_bundle(stage)
        if output.exists() or output.is_symlink():
            raise EvidenceError("evidence output appeared during collection")
        os.replace(stage, output)
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise
    return output


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    collect_parser = subparsers.add_parser(
        "collect", help="collect evidence for explicitly named installed packages"
    )
    collect_parser.add_argument(
        "--package",
        action="append",
        required=True,
        help="installed binary package name; repeat for each explicit package",
    )
    collect_parser.add_argument(
        "--documentation-root", default="/usr/share/doc", help=argparse.SUPPRESS
    )
    collect_parser.add_argument(
        "--output",
        required=True,
        help="new absolute directory for evidence.json and copied copyright notices",
    )
    verify_parser = subparsers.add_parser("verify", help="verify an evidence bundle offline")
    verify_parser.add_argument("bundle", help="evidence directory created by collect")
    args = parser.parse_args(argv)
    try:
        if args.command == "collect":
            result = collect(args.package, args.output, documentation_root=args.documentation_root)
            print(f"captured explicit-package evidence only; not release-ready: {result}")
        else:
            verify_bundle(args.bundle)
            print(
                "verified explicit-package identity and notices only; "
                "dependency closure/origin unverified; not release-ready"
            )
    except (EvidenceError, OSError) as error:
        print(f"apt evidence incomplete: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
