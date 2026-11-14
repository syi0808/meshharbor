# Pixal3D CUDA source snapshot

This directory preserves source from [`syi0808/pixal3d-cuda`](https://github.com/syi0808/pixal3d-cuda), pinned to commit `a4cb2b226cdb2bf3328eb280d4a02e510cc78e8c`. The full upstream `LICENSE` and source layout are retained under `upstream/`; **56 of the 59 upstream Git blobs** are present byte-for-byte. Per-file blob IDs and the exact exclusions are recorded in [`docs/pixal3d-source-inventory.json`](../../../docs/pixal3d-source-inventory.json).

The three omitted files are generated historical test evidence (`docs/evidence/t4-shoe-2026-11-11.acceptance.json`, `docs/evidence/t4-shoe-2026-11-11.report.json`, and `docs/evidence/t4-shoe-2026-11-11.tests.txt`). They are not runtime inputs or source. No weights, model checkpoints, user input images, generated meshes, or credentials are included. Both upstream notebooks have zero saved cell outputs and zero execution counts.

## Distribution and runtime boundary

The retained upstream MIT license names Noah Lyons and covers the upstream porting code only. It does not establish a MeshHarbor project-wide license or grant rights to separate models, weights, dependencies, or services. Third-party license/provenance records are in [`../THIRD-PARTY-LICENSES/`](../THIRD-PARTY-LICENSES/) and [`../THIRD-PARTY-NOTICES.md`](../THIRD-PARTY-NOTICES.md), including DINOv3, NAF, MoGe, TRELLIS.2, and credited port projects. DINOv3 has its own custom agreement; NAF checkpoint commercial inference terms remain unresolved.

The pinned files under `upstream/` remain a byte-preserved source snapshot; MeshHarbor does not execute them from this repository checkout. The opt-in manager in `crates/core` materializes the allowlisted CUDA source into the app-private data directory, installs an isolated Python 3.12/uv environment, and invokes the snapshot's local CLI on Linux x86_64 with an NVIDIA GPU. The build allowlist covers both CUDA and MLX source files: it verifies each file's size and SHA-256 against `crates/core/runtime_bundle_manifest.json`, whose own digest and entry count are pinned in `crates/core/build.rs`. Any other regular file outside ignored `__pycache__/` directories and `.pyc` files, or any changed source hash, fails the build; only verified staged bytes are embedded. Model downloads are a separate, explicitly confirmed action. See the [managed CUDA runtime guide](../../../docs/pixal3d-cuda-managed-runtime.md) for setup and limits.

The nested upstream deployment and CI files are preserved for provenance but are not activated by MeshHarbor: it does not deploy or call the upstream Modal service, and these nested workflows are not product workflows. The managed CUDA path remains experimental and still needs compatible-device, end-to-end validation.

The upstream MIT copyright notice is preserved verbatim. See the [project licensing status](../../../docs/licensing-status.md); do not present the backend or whole MeshHarbor repository as commercially cleared.
