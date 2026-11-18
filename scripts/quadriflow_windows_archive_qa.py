#!/usr/bin/env python3
"""Reconstruct the fixed, non-release Windows archive used for native QA.

Five local inputs are already published upstream/candidate objects. No downloads,
execution, signing, upload, product installer, or release operations occur here.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
from pathlib import Path
import tarfile
import zipfile

import quadriflow_notices as notices
import quadriflow_ustar_package as ustar

SOURCE = "810b7a0967c35b0dc85b4464e3835e26a756c967"
WORKFLOW_COMMIT = "9288f601578f8c9b5735a1b255b661f6ebcd1c50"
PLATFORM = "windows-x86_64"
EXPECTED_ARCHIVE_BYTES = 4_239_360
EXPECTED_ARCHIVE_SHA256 = "9037e53b8bdcc4192638bc5afdd5efa64a7b1c0ae0cc2e3aaa1731418bfee8f6"
EIGEN_SOURCE = "https://gitlab.com/libeigen/eigen/-/archive/5.0.1/eigen-5.0.1.tar.gz"
BOOST_PORT = (
    "https://github.com/microsoft/vcpkg/blob/"
    "177e8175121536d414f405ed8b3099b325a42638/ports/boost-cmake/portfile.cmake"
)
INPUT_PINS = {
    "candidate": (4 * 1024 * 1024, "cab09fe8a31e247f7c176e3a63eff074c07e581a44e50b36762f3bed4a9384d5"),
    "eigen": (16 * 1024 * 1024, "e9c326dc8c05cd1e044c71f30f1b2e34a6161a3b6ecf445d56b53ff1669e3dec"),
    "boost": (16 * 1024, "c9bff75738922193e67fa726fa225535870d2aa1059f91452c411736284ad566"),
    "pcg": (16 * 1024, "7757be3b65a08c1ba6314dfe39e9e82902aa763447d09b727f6ff32bb11c8c59"),
    "apache": (32 * 1024, "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"),
}
EXE_SHA = "e3742367c7df642bf8d194c55bf63e7eb39c2af74b9627ee1045bd2bb1687aa6"
BUNDLE_SHA = "9ee17ca527881f6940c40e937a694c85b9556e14a896e5d4da1452d59c31ce16"
COPYING = ("COPYING.README", "COPYING.APACHE", "COPYING.BSD", "COPYING.MINPACK", "COPYING.MPL2")


def digest(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def read_input(name: str, path: Path) -> bytes:
    cap, expected = INPUT_PINS[name]
    if path.is_symlink() or not path.is_file() or path.stat().st_size > cap:
        raise ValueError(f"invalid {name} input file")
    with path.open("rb") as stream:
        raw = stream.read(cap + 1)
    if len(raw) > cap or digest(raw) != expected:
        raise ValueError(f"{name} input SHA-256 mismatch")
    return raw


def unpack_reviewed_inputs(inputs):
    vendor, original, eigen = {}, {}, {}
    with zipfile.ZipFile(io.BytesIO(inputs["candidate"])) as candidate:
        def member(path, cap):
            entries = [entry for entry in candidate.infolist() if entry.filename == path]
            if len(entries) != 1 or entries[0].is_dir() or not 0 < entries[0].file_size <= cap:
                raise ValueError("candidate member missing, duplicated or invalid")
            return candidate.read(entries[0])

        vendor["quadriflow"] = {"LICENSE.txt": member("third-party-notices/LICENSE.txt", 64 * 1024)}
        vendor["lemon"] = {"LICENSE": member("third-party-notices/3rd/lemon-1.3.1/LICENSE", 64 * 1024)}
        vendor["maple"] = {"LICENSE": member("third-party-notices/3rd/MapleCOMSPS_LRB/LICENSE", 64 * 1024)}
        original["quadriflow.exe"] = member("quadriflow.exe", 4 * 1024 * 1024)
        original["provenance.json"] = member("provenance.json", 64 * 1024)
        original["provenance.sigstore.json"] = member("provenance.sigstore.json", 256 * 1024)
    if digest(original["quadriflow.exe"]) != EXE_SHA or digest(original["provenance.sigstore.json"]) != BUNDLE_SHA:
        raise ValueError("candidate executable or bundle did not match the reviewed hash")

    with tarfile.open(fileobj=io.BytesIO(inputs["eigen"]), mode="r:gz") as source:
        for name in COPYING:
            entry = source.getmember(f"eigen-5.0.1/{name}")
            if not entry.isfile() or not 0 < entry.size <= 64 * 1024:
                raise ValueError("invalid Eigen source notice")
            eigen[name] = source.extractfile(entry).read()
    pcg_header = inputs["pcg"]
    if not pcg_header.startswith(b"/*\n") or b"*/\n" not in pcg_header:
        raise ValueError("invalid pinned PCG header notice")
    pcg = {
        "NOTICE.pcg32-header": pcg_header.split(b"*/\n", 1)[0] + b"*/\n",
        "LICENSE.apache-2.0": inputs["apache"],
    }
    return vendor, original, eigen, pcg


def build(args):
    inputs = {name: read_input(name, getattr(args, name)) for name in INPUT_PINS}
    vendor, original, eigen, pcg = unpack_reviewed_inputs(inputs)
    output = args.output
    if output.exists() or output.is_symlink() or not output.parent.is_dir():
        raise ValueError("output must be a new directory under an existing parent")
    output = output.parent.resolve(strict=True) / output.name
    output.mkdir()
    notice_inputs = output / "notice-inputs"
    notice_inputs.mkdir()
    payload = output / "payload"
    payload.mkdir()
    packages = []

    def add(ident, name, version, manager, files, license=None, download=None):
        root = notice_inputs / ident
        root.mkdir()
        for filename, raw in files.items():
            (root / filename).write_bytes(raw)
        item = dict(id=ident, name=name, version=version, manager=manager,
                    root=str(root), notices=sorted(files))
        if license is not None:
            item["license_declared"] = license
        if download is not None:
            item["download_location"] = download
        packages.append(item)

    add("quadriflow", "QuadriFlow", SOURCE, "source", vendor["quadriflow"])
    add("lemon", "LEMON", "1.3.1", "source", vendor["lemon"], "BSL-1.0")
    add("maple", "MapleCOMSPS_LRB (candidate archive notice; not compiled)", SOURCE,
        "source", vendor["maple"])
    add("pcg32", "PCG32 (vendored header)", SOURCE, "source", pcg, "Apache-2.0")
    add("vcpkg-eigen3", "Eigen3", "5.0.1", "vcpkg", eigen, download=EIGEN_SOURCE)
    add("vcpkg-boost", "Boost (upstream notice used by vcpkg)", "1.92.0", "vcpkg",
        {"LICENSE_1_0.txt": inputs["boost"]}, "BSL-1.0", BOOST_PORT)

    evidence = output / "notice-evidence"
    notices.build(dict(source_commit=SOURCE, packages=packages), evidence, 1794751200,
                  archive_evidence=True)
    for filename, raw in original.items():
        (payload / filename).write_bytes(raw)
    archive_evidence = evidence / "archive-evidence"
    (payload / "THIRD-PARTY-NOTICES.md").write_bytes((archive_evidence / "THIRD-PARTY-NOTICES.md").read_bytes())
    historical_sbom = json.loads((archive_evidence / "sbom.spdx.json").read_bytes())
    for record in historical_sbom["files"]:
        record["checksums"] = [entry for entry in record["checksums"] if entry["algorithm"] == "SHA256"]
    (payload / "sbom.spdx.json").write_bytes(notices.canonical(historical_sbom))
    archive = output / "windows-rehearsal.tar"
    report = ustar.build_ustar(payload, archive, PLATFORM)
    if report["size_bytes"] != EXPECTED_ARCHIVE_BYTES or report["sha256"] != EXPECTED_ARCHIVE_SHA256:
        raise ValueError("reconstructed local archive bytes do not match the reviewed digest")

    tag = "quadriflow-v0.0.0-native-rehearsal-test"
    filename = f"quadriflow-{PLATFORM}.tar"
    files = {item["path"]: item for item in report["payloads"]}
    artifact = {
        "platform": PLATFORM,
        "archive": dict(filename=filename, format="tar", sha256=report["sha256"],
                        size_bytes=report["size_bytes"],
                        url=f"https://github.com/syi0808/meshharbor/releases/download/{tag}/{filename}"),
        "workflow": dict(repository="syi0808/meshharbor",
                         path=".github/workflows/quadriflow-candidate-build.yml",
                         run_id=43721747855, run_attempt=1, commit=WORKFLOW_COMMIT),
        "payload": dict(executable=files["bin/quadriflow.exe"],
                        third_party_notices=files["THIRD-PARTY-NOTICES.md"],
                        sbom=files["sbom.spdx.json"], provenance=files["provenance.json"],
                        provenance_signature=files["provenance.sigstore.json"]),
        "attestation_bundle_base64": base64.b64encode(original["provenance.sigstore.json"]).decode("ascii"),
    }
    manifest = dict(schema_version=1, backend="quadriflow",
                    source=dict(repository="https://github.com/hjwdzh/QuadriFlow", commit=SOURCE),
                    release_tag=tag, artifacts=[artifact])
    (output / "local-only-manifest.json").write_bytes(notices.canonical(manifest))
    print(json.dumps({"archive_sha256": report["sha256"], "platform": PLATFORM,
                      "test_only": True, "release_ready": False}, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in INPUT_PINS:
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    build(parser.parse_args())


if __name__ == "__main__":
    main()
