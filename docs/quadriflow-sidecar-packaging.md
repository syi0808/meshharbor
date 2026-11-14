# QuadriFlow package preparation

This is preparatory evidence tooling, not a released package. CLI, REST, MCP, and Studio expose backend status and a guarded installation request, but the request currently fails closed; it is not a usable download/install workflow.

The pinned upstream source is `hjwdzh/QuadriFlow` at `810b7a0967c35b0dc85b4464e3835e26a756c967`. Three platform-specific synthetic-cube smoke builds have passed in isolated GitHub Actions jobs; those older smoke artifacts remain unsigned temporary evidence. A separate manual workflow can build and attest a candidate executable on each platform, then transfer it to a fresh hosted runner job for verification. Those temporary candidates are still not trusted distribution packages and must not be installed by the application.

The release manifest currently contains no artifacts. The public CLI/REST/MCP/Studio install route checks a release-verification gate before calling the lower-level transactional installer. With the current empty manifest, install requests do not download, create a QuadriFlow backend directory, or install anything. The lower-level installer supports future uncompressed POSIX USTAR packages with bounded system-curl downloads, strict member/path checks, SHA-256 verification, owner-private staging, immutable versions, atomic activation markers, and explicit rollback; fake-downloader tests do not access the network or execute QuadriFlow.

A future package needs an exact source and workflow commit, per-platform archive and payload checksums, complete third-party notices, an independently validated SBOM, and verifiable provenance/signature material. The product now verifies a manifest-pinned Sigstore bundle and its signed SLSA source/material policy before issuing the installer capability. A hash-pinned or provenance-verified artifact is not legal clearance or release approval. The manual candidate workflow's attestation applies only to the executable; it does not attest a complete install archive or prove dependency closure, license compliance, or release approval. The candidate workflow therefore cannot populate the release manifest by itself.

## Manual candidate build and attestation (not a release)

`.github/workflows/quadriflow-candidate-build.yml` has only a `workflow_dispatch` trigger. A maintainer must choose the main branch and check `confirm_candidate`; jobs on other refs or without the explicit input are skipped. The workflow builds the pinned QuadriFlow source and does not execute the generated executable or run the synthetic-cube smoke test. It creates a GitHub artifact attestation for the executable with the SHA-pinned official `actions/attest` action, then uploads an unverified short-lived transfer artifact. A separate GitHub-hosted job checks out the exact same workflow commit, downloads the candidate without executing it, verifies the local bundle with the runner-provided GitHub CLI, and uploads verified temporary evidence with seven-day retention. This keeps build tools and changes made by compiling upstream code out of the verifier job. The verifier trusts the GitHub-hosted runner image and its preinstalled `gh` binary; the script checks the CLI version but does not independently hash that binary. It does not create a GitHub Release or write to the product manifest. The attestation itself is stored by GitHub as public repository metadata.

`scripts/quadriflow_attestation_verify.py` invokes the clean verifier job's GitHub CLI without a shell and fails closed unless the verified output matches the fixed policy: MeshHarbor's repository and numeric IDs, `refs/heads/main`, exact source and workflow commits, the candidate workflow path, the GitHub Actions OIDC issuer, a `workflow_dispatch` trigger, GitHub-hosted runner, SLSA provenance v1, and the executable's SHA-256 subject. It obtains the Sigstore trusted-root JSONL through `gh attestation trusted-root` (TUF-verified by the CLI), uses it only for the local bundle verification, and additionally checks the CLI's certificate and statement summary before retaining the bundle. The CLI must be version 2.96.0 or newer in the 2.x series. Unit tests mock all CLI invocations; they test command-policy construction and rejection logic, but do not cryptographically verify a bundle or check the trust root offline. The candidate workflow itself performs real Sigstore verification on the hosted runners, but this evidence cannot mint the product's in-process installer capability.


The certificate's `sourceRepositoryDigest` is the MeshHarbor workflow-source commit, not the pinned QuadriFlow upstream commit. The in-process verifier binds the signed SLSA materials to both commits, the checked-in build recipe, workflow identity, run/attempt, and executable digest. It includes an offline cryptographic regression fixture from upstream Sigstore tests. This verifies provenance claims, not that the build is reproducible, that native dependencies are fully inventoried, or that the source is legally cleared; the empty product manifest and disabled installer gate stay in place.

The resulting files are candidate evidence only. `provenance.json` is diagnostic build metadata, and `verification.json` is a local verifier summary, not a second signature. The candidate-status file remains `installable=false`, `release_ready=false`, and `dependency_closure_verified=false`. In particular, this candidate workflow does not produce a complete SBOM or reviewed dependency notices. The product verifier is integrated, but the release manifest stays empty and installer requests remain unavailable until a complete, reviewed package passes all independent release gates.

Action references in the candidate workflow are pinned to immutable commits: checkout v4.2.2 (`11bd71901bbe5b1630ceea73d27597364c9af683`), setup-python v5.6.0 (`a26af69be951a213d495a4c3e4e4022e16d87065`), attest v4.2.2 (`1e69f48acb82d1966a394da916b4c1698aa569d6`), upload-artifact v4.6.2 (`ea165f8d65b6e75b540449e92b4886f43607fa02`), and download-artifact v4.3.0 (`d3f86a106a0bac45b974a628896c90dbdf5c8093`). The source commit and build flags are fixed. Hosted runner images and apt/Homebrew/vcpkg dependency state are still mutable and are recorded only as diagnostics. This is not a reproducible-build claim; full dependency closure and platform packaging remain release blockers.

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

The older smoke workflow installs native dependencies from moving apt, Homebrew, and vcpkg state and uses version-tag action references. The manual candidate workflow pins its actions and Python version, but native compiler/dependency state and hosted runner images remain mutable. Recorded tool/package versions are diagnostic, not lockfiles. A release pipeline therefore still needs pinned compiler/CMake versions, a pinned vcpkg baseline or exact formula/package snapshots, a pinned Linux image and packages, deterministic archive creation, and repeat builds with matching hashes before it can claim reproducibility.

The source notice scan does not prove the complete per-platform dependency closure, including compiler runtime libraries and linked native packages. Complete notice collection, independently validated SBOMs, provenance/signature verification, and legal review are still release gates. No project-level license is selected here.
