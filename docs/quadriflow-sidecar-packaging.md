# QuadriFlow package preparation

This is preparatory evidence tooling, not a released package. CLI, REST, MCP, and Studio expose backend status and a guarded installation request, but the request currently fails closed; it is not a usable download/install workflow.

The pinned upstream source is `hjwdzh/QuadriFlow` at `810b7a0967c35b0dc85b4464e3835e26a756c967`. Three platform-specific synthetic-cube builds have passed in isolated GitHub Actions jobs. The resulting Actions artifacts are unsigned, temporary evidence with short retention. They are not trusted distribution packages and must not be downloaded or installed by the application.

The release manifest currently contains no artifacts. The public CLI/REST/MCP/Studio install route checks a release-verification gate before calling the lower-level transactional installer. With the current empty manifest, install requests do not download, create a QuadriFlow backend directory, or install anything. The lower-level installer supports future uncompressed POSIX USTAR packages with bounded system-curl downloads, strict member/path checks, SHA-256 verification, owner-private staging, immutable versions, atomic activation markers, and explicit rollback; fake-downloader tests do not access the network or execute QuadriFlow.

A future package needs an exact source and workflow commit, per-platform archive and payload checksums, complete third-party notices, an independently validated SBOM, and verifiable provenance/signature material. The code verifies hashes against the application-bundled manifest, but does not verify Sigstore provenance signatures or claims. A hash-pinned artifact is not a legal clearance or a release approval. The current smoke workflow still produces unsigned CI evidence without the complete package payload, so it cannot populate the release manifest.

## Notice/SBOM evidence helper

`scripts/quadriflow_notices.py` is an offline, Python standard-library-only tool. It accepts a caller-supplied inventory and writes to a new output directory:

- Original, byte-preserved notice files under `third-party-notices/<package-id>/`.
- `THIRD-PARTY-NOTICES.md` index and `notice-inventory.json`.
- SPDX 2.3-shaped `sbom.spdx.json` plus `SHA256SUMS` (which excludes its own checksum).

Example inventory (use only after checking the actual source checkout and installed package versions):

```json
{
  "source_commit": "810b7a0967c35b0dc85b4464e3835e26a756c967",
  "packages": [{
    "id": "quadriflow",
    "name": "QuadriFlow",
    "version": "810b7a0967c35b0dc85b4464e3835e26a756c967",
    "manager": "source",
    "root": "/canonical/real/checkout",
    "notices": ["LICENSE.txt"],
    "license_declared": "NOASSERTION"
  }]
}
```

Run it with an explicit stable epoch and a destination that does not exist:

```sh
OUT=$(python3 -c 'from pathlib import Path; print(Path("/tmp").resolve() / "qf-notices")')
python3 scripts/quadriflow_notices.py inventory.json "$OUT" --source-date-epoch 1760000000
```

The source revision field is a caller assertion; the tool does not inspect Git. It requires each dependency to be listed explicitly with its installed version and reviewed notice paths. Candidate discovery for source/Homebrew is bounded; apt/vcpkg adapters use explicit package names and conventional copyright paths. It never queries a package manager, fetches license text, resolves dependency closure, or makes license conclusions.

Inputs and files are bounded (512 packages/notices, 2 MiB per notice, 16 MiB total notice bytes, 20,000 candidate scan entries). It rejects symlinks, traversal, control characters, binary/empty content, nonportable names, and case-insensitive output collisions. It creates a new output directory exclusively and does not overwrite one. Output parents and inventory roots must be canonical real directories; on macOS, resolve the `/tmp` alias before using it. Deterministic JSON/checksum content uses the supplied timestamp; directory metadata and native compiler outputs are not normalized, so this does not make the overall build reproducible.

The helper intentionally records `release_ready=false` and `dependency_closure_verified=false`. Its SPDX-shaped output is not checked by an external SPDX validator, license-expression grammar is not fully parsed, and no legal approval is implied. A release pipeline must review the actual linked dependency closure and independently validate the SPDX output.

## Reproducibility and release gates

The current smoke workflow installs native dependencies from moving apt, Homebrew, and vcpkg state. The workflow's GitHub Actions references are version tags rather than immutable commit SHAs. Recorded tool/package versions are diagnostic, not lockfiles. A release pipeline therefore needs immutable action revisions, pinned compiler/CMake versions, a pinned vcpkg baseline or exact formula/package snapshots, a pinned Linux image and packages, deterministic archive creation, and repeat builds with matching hashes before it can claim reproducibility.

The source notice scan does not prove the complete per-platform dependency closure, including compiler runtime libraries and linked native packages. Complete notice collection, independently validated SBOMs, provenance/signature verification, and legal review are still release gates. No project-level license is selected here.
