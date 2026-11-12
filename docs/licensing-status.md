# Licensing and runtime gate

This file records the checks made for the initial implementation. It is not a legal opinion or a completed transitive SBOM. Do not present the full MeshHarbor runtime as commercially cleared until every installed package, native library, model, and weight revision is reviewed.

## Project license

- The destination repository was empty and had no license. No root `LICENSE` is present; the owner has not selected the project license.
- The user explicitly authorized copying relevant code from `syi0808/glb-mesh-refiner` into the named public repository. That transfer does not select a MeshHarbor project license or grant the public an open-source reuse license. The source repository has no root `LICENSE`; preserve this provenance and do not claim commercial or third-party redistribution clearance.

## Native mesh worker

- The initial worker includes only `src/refine.cpp` and its core tests from `syi0808/glb-mesh-refiner` at commit `35383ddab8bbe491b98264b42892c42684c7d514`.
- The core worker has no third-party build inputs. It compiles with C++17 and uses only the standard library.
- The optional atlas baker is not included. It brings xatlas, meshoptimizer, TinyBVH, and stb notices, has narrower texture/material support, and is experimental. If it is integrated later, preserve all embedded and standalone notices, including xatlas's BSD-style notice and TinyBVH's current header copyright.

## Pixal3D candidates

- `syi0808/pixal3d-cuda` and `syi0808/pixal3d-mlx` porting code carry MIT licenses with copyright notice for Noah Lyons. Their licenses explicitly do not cover upstream model weights. Keep those notices when porting code is included.
- Pixal3D's public code and its single-view checkpoint are marked MIT in the reviewed upstream notice/card, but the multi-view snapshot was not independently verified.
- NAF is Apache-2.0 at repository level, but its RoPE code contains the DINOv3 license header. The exact NAF release checkpoint's commercial inference scope is still unresolved in upstream issue 13.
- DINOv3 uses a separate gated/custom agreement for code and weights. It is not an NC-only license, but it is not MIT/Apache and its access terms must be honored.
- The MLX generation path defaults to CGAL-based hole filling; the relevant CGAL components are GPL-3.0-or-later or separately commercially licensed. This helper is not bundled here.
- MLX's unpinned `rembg` call can select BRIA RMBG-2.0 in current releases, whose model card is CC BY-NC 4.0 absent a separate commercial grant. The CUDA port explicitly selects U²-Net, but the full model/dependency chain has not passed an SBOM review.
- The original upstream Pixal3D pipeline configuration also selects BRIA RMBG-2.0. Do not wholesale execute that configuration.

Both Pixal3D backends therefore remain `pending`; this prototype exposes them as non-installable candidates and does not fetch code, weights, or dependencies. Only a reviewed, pinned runtime manifest with approved licenses can enable installation.

## Sources

- [CUDA port license](https://github.com/syi0808/pixal3d-cuda/blob/main/LICENSE) · [CUDA dependencies](https://github.com/syi0808/pixal3d-cuda/blob/main/pyproject.toml)
- [MLX port license](https://github.com/syi0808/pixal3d-mlx/blob/main/LICENSE) · [MLX dependencies](https://github.com/syi0808/pixal3d-mlx/blob/main/pyproject.toml)
- [NAF unresolved checkpoint question](https://github.com/valeoai/NAF/issues/13) · [DINOv3 license](https://github.com/facebookresearch/dinov3/blob/main/LICENSE.md)
- [CGAL licensing](https://www.cgal.org/license.html) · [BRIA RMBG-2.0 terms](https://huggingface.co/briaai/RMBG-2.0)
- [Refiner source at pinned revision](https://github.com/syi0808/glb-mesh-refiner/tree/35383ddab8bbe491b98264b42892c42684c7d514)
