"""Offline notice inventory + deterministic SPDX 2.3 evidence, never release clearance.

Inventory is caller-owned JSON: source_commit (exact pinned revision), packages list
of {id, name, version, manager, root, notices, license_declared?, download_location?}.
root is an explicit absolute real directory; notices are relative POSIX file paths.
manager is source/vcpkg/apt/homebrew. No network, subprocess, or executable loading.
Source commit is a caller assertion, not independently verified by this helper.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
from datetime import datetime, timezone
import uuid

PIN = "810b7a0967c35b0dc85b4464e3835e26a756c967"
MAX_FILES = 512
MAX_FILE_BYTES = 2 * 1024 * 1024
MAX_TOTAL_BYTES = 16 * 1024 * 1024
MAX_SCAN_ENTRIES = 20_000
MAX_METADATA_TEXT = 4_096
MAX_NOTICE_INDEX_BYTES = 2 * 1024 * 1024
ID = re.compile(r"^[a-z0-9][a-z0-9.-]{0,79}$")
PACKAGE_NAME = re.compile(r"^[a-z0-9][a-z0-9.+-]{0,119}$")
SPDX_EXPRESSION_CHARS = re.compile(r"^[A-Za-z0-9.+() -]+$")
RESERVED = {
    "CON",
    "PRN",
    "AUX",
    "NUL",
    *(f"COM{i}" for i in range(1, 10)),
    *(f"LPT{i}" for i in range(1, 10)),
}


def canonical(value):
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def safe_root(value):
    if not isinstance(value, str) or not value:
        raise ValueError("root must be an absolute canonical path")
    root = Path(value)
    if not root.is_absolute() or root != root.resolve():
        raise ValueError("root must be an absolute canonical path without symlinks")
    if not root.is_dir():
        raise ValueError("root is not a directory")
    return root


def safe_file(root, value):
    if (
        not isinstance(value, str)
        or "\\" in value
        or any(ord(c) < 32 or ord(c) == 127 for c in value)
    ):
        raise ValueError("notice path must be relative POSIX text")
    rel = PurePosixPath(value)
    if rel.is_absolute() or not rel.parts or any(p in ("..", ".") for p in value.split("/")):
        raise ValueError("unsafe notice path")
    if any(
        not p
        or any(c in p for c in '<>:"|?*')
        or p.endswith((" ", "."))
        or p.split(".")[0].rstrip(" .").upper() in RESERVED
        for p in value.split("/")
    ):
        raise ValueError("unsafe notice path component")
    target = root
    for component in rel.parts:
        target /= component
        if target.is_symlink():
            raise ValueError("notice symlinks are not followed")
    mode = target.stat().st_mode
    if not stat.S_ISREG(mode):
        raise ValueError("notice is not a regular file")
    return target, rel.as_posix()


def read_notice(root, relative):
    path, rel = safe_file(root, relative)
    # Explicit roots must remain quiescent; this is not a sandbox against concurrent
    # hostile filesystem mutation. Bounded read prevents oversized allocations.
    with path.open("rb") as handle:
        data = handle.read(MAX_FILE_BYTES + 1)
    if len(data) > MAX_FILE_BYTES:
        raise ValueError("notice exceeds per-file limit")
    if not data or b"\0" in data:
        raise ValueError("notice is empty or binary")
    data.decode("utf-8", errors="strict")
    return rel, data


def discover(root_value, manager, package=None):
    """Candidate discovery only; caller reviews/pins paths in the inventory.

    vcpkg: root=<installed>/<triplet>, package=<port>, share/<port>/copyright.
    apt: root=/usr/share/doc, package=<binary-package>, <package>/copyright.
    homebrew: root=<canonical Cellar>/<formula>/<version>; bounded notice-name scan.
    source: root=<canonical pinned checkout>; same bounded scan, excluding .git.
    Canonicalize Homebrew opt and /tmp aliases in the caller, not silently here.
    """
    root = safe_root(root_value)
    if manager in ("vcpkg", "apt"):
        if not isinstance(package, str) or not PACKAGE_NAME.fullmatch(package):
            raise ValueError("explicit safe package name required")
        rel = f"share/{package}/copyright" if manager == "vcpkg" else f"{package}/copyright"
        safe_file(root, rel)
        return [rel]
    if manager not in ("source", "homebrew"):
        raise ValueError("unsupported manager")
    found, scanned = [], 0

    def fail_scan(error):
        raise ValueError("notice discovery could not scan the complete requested tree") from error

    for directory, dirs, files in os.walk(root, followlinks=False, onerror=fail_scan):
        dirs[:] = sorted(d for d in dirs if d != ".git")
        for name in dirs + sorted(files):
            scanned += 1
            if scanned > MAX_SCAN_ENTRIES:
                raise ValueError("notice discovery exceeds entry limit")
            path = Path(directory) / name
            if path.is_symlink():
                raise ValueError("symlink in notice discovery tree; use explicit reviewed paths")
            if name in files and name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT")):
                found.append(path.relative_to(root).as_posix())
                if len(found) > MAX_FILES:
                    raise ValueError("too many notice candidates")
    return sorted(found)


def safe_metadata_text(value, field):
    if (
        not isinstance(value, str)
        or not value
        or len(value) > MAX_METADATA_TEXT
        or any(ord(char) < 32 or ord(char) == 127 for char in value)
    ):
        raise ValueError(f"invalid package {field}")
    return value


def build(inventory, output, epoch):
    if not isinstance(inventory, dict):
        raise ValueError("inventory must be a JSON object")
    if set(inventory) != {"source_commit", "packages"}:
        raise ValueError("inventory has missing or unknown fields")
    if not isinstance(epoch, int) or isinstance(epoch, bool) or not 0 <= epoch <= 4_102_444_800:
        raise ValueError("source date epoch must be an integer between 1970 and 2100")
    if inventory.get("source_commit") != PIN:
        raise ValueError("unexpected QuadriFlow source revision")
    packages = inventory.get("packages")
    if not isinstance(packages, list) or not packages or len(packages) > MAX_FILES:
        raise ValueError("nonempty bounded package inventory required")
    required_package_fields = {"id", "name", "version", "manager", "root", "notices"}
    allowed_package_fields = required_package_fields | {"license_declared", "download_location"}
    if any(not isinstance(package, dict) or not isinstance(package.get("id"), str) for package in packages):
        raise ValueError("each package must be an object with a string id")
    for package in packages:
        if not required_package_fields.issubset(package) or set(package) - allowed_package_fields:
            raise ValueError("package inventory has missing or unknown fields")
        if not ID.fullmatch(package["id"]):
            raise ValueError("duplicate/unsafe package id")
    packages = sorted(packages, key=lambda package: package["id"])

    created = datetime.fromtimestamp(epoch, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    payloads, records, spdx_packages, spdx_files, relationships = {}, [], [], [], []
    seen, total, warnings = set(), 0, []
    portable_paths = set()
    for package in packages:
        ident = package["id"]
        if ident in seen:
            raise ValueError("duplicate/unsafe package id")
        seen.add(ident)
        manager = package.get("manager")
        if manager not in ("source", "vcpkg", "apt", "homebrew"):
            raise ValueError("unsupported package manager")
        name = safe_metadata_text(package.get("name"), "name")
        version = safe_metadata_text(package.get("version"), "version")
        root = safe_root(package.get("root"))
        notices = package.get("notices")
        if (
            not isinstance(notices, list)
            or not notices
            or len(notices) > MAX_FILES
            or any(not isinstance(path, str) for path in notices)
        ):
            raise ValueError("every declared package requires a bounded list of text notice paths")
        if len(notices) != len(set(notices)):
            raise ValueError("duplicate notice path")
        package_id = "SPDXRef-Package-" + ident
        declared = package.get("license_declared", "NOASSERTION")
        # The expression grammar and legal meaning are not evaluated here.
        if (
            not isinstance(declared, str)
            or not declared.strip()
            or declared != declared.strip()
            or not SPDX_EXPRESSION_CHARS.fullmatch(declared)
            or re.search(r"LicenseRef-", declared, re.IGNORECASE)
        ):
            raise ValueError("license expression missing, unsafe, or unresolved LicenseRef")
        if declared == "NOASSERTION":
            warnings.append(f"{ident}: license expression not asserted")
        download_location = package.get("download_location", "NOASSERTION")
        if download_location != "NOASSERTION":
            download_location = safe_metadata_text(download_location, "download_location")

        record = {"id": ident, "name": name, "version": version, "manager": manager}
        record["license_declared"] = declared
        record["notices"] = []
        for relative in sorted(notices):
            rel, data = read_notice(root, relative)
            total += len(data)
            if len(payloads) >= MAX_FILES or total > MAX_TOTAL_BYTES:
                raise ValueError("notice bundle exceeds global limits")
            destination = f"third-party-notices/{ident}/{rel}"
            if destination.casefold() in portable_paths:
                raise ValueError("case-insensitive notice path collision")
            portable_paths.add(destination.casefold())
            payloads[destination] = data
            digest = hashlib.sha256(data).hexdigest()
            record["notices"].append({"path": destination, "sha256": digest, "bytes": len(data)})
            file_id = "SPDXRef-File-" + hashlib.sha256(destination.encode()).hexdigest()[:32]
            spdx_files.append(
                {
                    "SPDXID": file_id,
                    "fileName": "./" + destination,
                    "checksums": [{"algorithm": "SHA256", "checksumValue": digest}],
                    "licenseConcluded": "NOASSERTION",
                    "licenseInfoInFile": ["NOASSERTION"],
                    "copyrightText": "NOASSERTION",
                }
            )
            relationships.append(
                {"spdxElementId": package_id, "relationshipType": "CONTAINS", "relatedSpdxElement": file_id}
            )
        records.append(record)
        spdx_packages.append(
            {
                "SPDXID": package_id,
                "name": name,
                "versionInfo": version,
                "downloadLocation": download_location,
                "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION",
                "licenseDeclared": declared,
                "copyrightText": "NOASSERTION",
                "comment": "Declared build-input inventory; notice files only, not binary dependency closure.",
            }
        )
        relationships.append(
            {"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES", "relatedSpdxElement": package_id}
        )

    report = {
        "schema_version": 1,
        "source_commit": PIN,
        "source_commit_verification": "caller_assertion_only",
        "release_ready": False,
        "dependency_closure_verified": False,
        "scope": "Explicit inventory only; does not infer linked libraries, legal clearance, or signature trust.",
        "created": created,
        "packages": records,
        "warnings": warnings,
    }
    namespace_hash = hashlib.sha256(canonical(report)).hexdigest()
    sbom = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": "QuadriFlow sidecar declared build inputs and notice inventory",
        "documentNamespace": "https://meshharbor.invalid/spdx/" + str(uuid.uuid5(uuid.NAMESPACE_URL, namespace_hash)),
        "creationInfo": {"creators": ["Tool: MeshHarbor-notice-evidence-1"], "created": created},
        "packages": spdx_packages,
        "files": spdx_files,
        "relationships": relationships,
        "comment": "Incomplete release evidence. Explicit inputs only; not an automated license conclusion or dependency-closure certification.",
    }

    notice_index = [
        "# Third-party notices",
        "",
        "This index accompanies byte-preserved notice texts under the third-party-notices directory.",
        "It is incomplete unless every build input is explicitly inventoried and independently reviewed.",
        "Presence of a notice does not establish a license conclusion or redistribution approval.",
        "",
    ]
    for record in records:
        notice_index.extend(
            [
                f"## Package {record['id']}",
                "",
                f"Notice files: third-party-notices/{record['id']}/ (see notice-inventory.json for exact paths and hashes).",
                "",
            ]
        )
    index_bytes = ("\n".join(notice_index).rstrip() + "\n").encode("utf-8")
    if len(index_bytes) > MAX_NOTICE_INDEX_BYTES:
        raise ValueError("third-party notice index exceeds size limit")
    payloads["THIRD-PARTY-NOTICES.md"] = index_bytes
    payloads["notice-inventory.json"] = canonical(report)
    payloads["sbom.spdx.json"] = canonical(sbom)
    payloads["SHA256SUMS"] = "".join(
        f"{hashlib.sha256(data).hexdigest()}  {name}\n" for name, data in sorted(payloads.items())
    ).encode("utf-8")

    output = Path(output)
    if not output.is_absolute() or not output.parent.is_dir() or output.parent != output.parent.resolve():
        raise ValueError("output must be a new path under an existing canonical directory")
    if output.exists() or output.is_symlink():
        raise FileExistsError(output)
    output.mkdir(parents=False, exist_ok=False)
    try:
        for name, data in sorted(payloads.items()):
            path = output / name
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("xb") as handle:
                handle.write(data)
            path.chmod(0o644)
            os.utime(path, (epoch, epoch))
    except Exception:
        shutil.rmtree(output)
        raise
    return report


def strict_json_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON object field")
        result[key] = value
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inventory", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--source-date-epoch", type=int, required=True)
    args = parser.parse_args()
    with args.inventory.open("rb") as handle:
        raw = handle.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        parser.error("inventory exceeds 1 MiB")
    inventory = json.loads(raw, object_pairs_hook=strict_json_object)
    report = build(inventory, args.output, args.source_date_epoch)
    print(json.dumps({"packages": len(report["packages"]), "release_ready": False}))


if __name__ == "__main__":
    main()
