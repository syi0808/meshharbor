# Pixal3D CUDA source snapshot

This directory preserves source from [`syi0808/pixal3d-cuda`](https://github.com/syi0808/pixal3d-cuda), pinned to commit `a4cb2b226cdb2bf3328eb280d4a02e510cc78e8c`. The full upstream `LICENSE` and source layout are retained under `upstream/`; **56 of the 59 upstream Git blobs** are present byte-for-byte. Per-file blob IDs and the exact exclusions are recorded in [`docs/pixal3d-source-inventory.json`](../../../docs/pixal3d-source-inventory.json).

The three omitted files are generated historical test evidence (`docs/evidence/t4-shoe-2026-11-11.acceptance.json`, `docs/evidence/t4-shoe-2026-11-11.report.json`, and `docs/evidence/t4-shoe-2026-11-11.tests.txt`). They are not runtime inputs or source. No weights, model checkpoints, user input images, generated meshes, or credentials are included. Both upstream notebooks have zero saved cell outputs and zero execution counts.

## Distribution and runtime boundary

The retained upstream MIT license names Noah Lyons and covers the upstream porting code only. It does not establish a MeshHarbor project-wide license or grant rights to separate models, weights, dependencies, or services. Third-party license/provenance records are in [`../THIRD-PARTY-LICENSES/`](../THIRD-PARTY-LICENSES/) and [`../THIRD-PARTY-NOTICES.md`](../THIRD-PARTY-NOTICES.md), including DINOv3, NAF, MoGe, TRELLIS.2, and credited port projects. DINOv3 has its own custom agreement; NAF checkpoint commercial inference terms remain unresolved.

This tree is a dormant source archive, **not an installable or enabled MeshHarbor runtime**. MeshHarbor does not execute its Python, install its dependencies, retrieve weights, or call its CUDA/Modal services. Upstream deployment and CI files remain nested under `upstream/` and are not active MeshHarbor workflows. A complete pinned runtime dependency/license review and compatible-platform QA are still required before execution or distribution of a runtime package.

The upstream MIT copyright notice is preserved verbatim. See the [project licensing status](../../../docs/licensing-status.md); do not present the backend or whole MeshHarbor repository as commercially cleared.
