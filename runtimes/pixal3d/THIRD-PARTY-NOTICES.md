# Pixal3D source snapshot third-party notices

These notices concern the preserved source snapshots in `cuda/upstream/` and `mlx/upstream/`. They are not a license for the MeshHarbor project as a whole. The code's exact upstream file hashes and origins are in [`docs/pixal3d-source-inventory.json`](../../docs/pixal3d-source-inventory.json).

## Backend port code

Both `syi0808/pixal3d-cuda` (`a4cb2b226cdb2bf3328eb280d4a02e510cc78e8c`) and `syi0808/pixal3d-mlx` (`3a832ebf5f346a7eafb1ba40ca2926cfbd0bb2d6`) retain their complete upstream MIT `LICENSE` files, naming Noah Lyons. These licenses cover the original port repositories as stated by their authors; they do not automatically relicense third-party-derived material, model weights, dependencies, or the rest of MeshHarbor.

## Referenced and adapted projects

License copies are preserved in `THIRD-PARTY-LICENSES/`. The referenced source projects' own notices control the portions to which they apply; file-level lineage has not been independently adjudicated, so this list is attribution and license preservation, not a claim that every line is MIT-cleared.

| Project / component | Preserved notice | Reference / revision for the notice copy |
|---|---|---|
| TencentARC Pixal3D | `THIRD-PARTY-LICENSES/TencentARC-Pixal3D-LICENSE` | `TencentARC/Pixal3D`, commit `f7cf38429b0bd264f1995f0f8743a88b1c728b94`; MIT source license. Model/checkpoint rights are separate and no weights are included. |
| Meta DINOv3 | `THIRD-PARTY-LICENSES/DINOv3-LICENSE.md` | `facebookresearch/dinov3`, commit `6876159a11b4df116f30f667f8c9888617df0751`; custom DINO Materials Agreement, not a standard MIT license. Relevant source includes DINOv3 feature implementations. Its use/distribution restrictions and gated weight access need separate review. |
| Valeo.ai NAF | `THIRD-PARTY-LICENSES/NAF-LICENSE` | `valeoai/NAF`, commit `37f2dfc180f2de53d98bd601109c0da0dd6b0f43`; Apache License 2.0. The NAF release checkpoint's commercial inference scope remains unresolved upstream. |
| Microsoft MoGe | `THIRD-PARTY-LICENSES/MoGe-LICENSE` | `microsoft/MoGe`, commit `74fbce054ebed49800de42d0ad0e83495065719a`; the license file contains MIT and Apache-2.0 terms. CUDA code pins a MoGe model revision separately. |
| Microsoft TRELLIS.2 | `THIRD-PARTY-LICENSES/TRELLIS2-LICENSE` | Microsoft MIT notice, preserved for related conversion/extraction code. |
| `lyonsno/pixal3d-mlx` | `THIRD-PARTY-LICENSES/lyonsno-pixal3d-mlx-LICENSE` | Commit `abc9a2e9f18568887dab7cba9814ea101bd0a430`; MIT. |
| `shivampkumar/trellis-mac` | `THIRD-PARTY-LICENSES/trellis-mac-LICENSE` | Commit `d58628f4f5b9c3de8274cb110074154f4b31cef2`; MIT. |
| `pedronaugusto/trellis2-apple` | `THIRD-PARTY-LICENSES/trellis2-apple-LICENSE` | Commit `c873bde81ebe1e56535c6caddb4b21267364a923`; MIT. |
| `pawel-mazurkiewicz/Pixal3D-mac` | `THIRD-PARTY-LICENSES/Pixal3D-mac-LICENSE` | Commit `0be9e69a729432323dfb60a418801e5e9e7a1437`; MIT. |

The MLX tree also retains C++ source for its CGAL bridge, but no CGAL library/header/binary. Building or distributing that separate helper requires resolving CGAL's GPL-3.0-or-later or commercial licensing and the full native dependency closure. The code stays dormant and is not part of the MeshHarbor runtime.

No model weights, checkouts, DINOv3 files, NAF checkpoints, generated meshes/images, credentials, or Python wheels were copied. Do not run the vendored installation/download/deployment instructions or activate nested CI/workflows as part of MeshHarbor setup. See [`docs/licensing-status.md`](../../docs/licensing-status.md) for remaining runtime and release gates.
