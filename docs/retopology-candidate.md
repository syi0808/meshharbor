# Experimental retopology

## Product status

MeshHarbor's `mesh.retopology` path is now a real QuadriFlow quad-remeshing candidate, not the separate isotropic triangle-remesher prototype.

The operation is wired through the Rust core, REST job API, MCP tool, CLI, and Studio, but it is unavailable in the current product build. Runtime code never auto-discovers or executes an adjacent `quadriflow` executable. A managed sidecar can be resolved only when an in-process verifier accepts its release attestation and the executable digest matches the trusted bundled manifest. The in-process verifier is integrated, but the manifest is empty and no approved sidecar is listed; readiness, install, and rollback remain fail-closed. The approved candidate run [#43721747855](https://github.com/syi0808/meshharbor/actions/runs/43721747855) completed the three platform build-pair, signing, and verification gates. Its Linux and macOS signed-digest native cube runs and MeshHarbor core geometry validators passed ([Linux job](https://github.com/syi0808/meshharbor/actions/runs/43721747855/job/132848222187), [macOS job](https://github.com/syi0808/meshharbor/actions/runs/43721747855/job/132848222186)). The original Windows native job failed its SLSA-material check before executing ([job](https://github.com/syi0808/meshharbor/actions/runs/43721747855/job/132848222181)); a read-only follow-up [run #43731453754](https://github.com/syi0808/meshharbor/actions/runs/43731453754) verified and executed the same original Windows artifact on a synthetic cube, but did not run the MeshHarbor core output validator. No textured appearance check was made. These temporary candidate results do not alter the empty product manifest. CLI (`meshharbor backend status` / `meshharbor backend install quadriflow`), REST (`GET /api/v1/backends/quadriflow`, `POST /api/v1/backends/quadriflow/install`), MCP tools, and Studio expose backend status and an install request, but the install action remains disabled/rejected.

A completed job stores two outputs together:
- A triangle-only GLB preview for viewing.
- The exact validated QuadriFlow OBJ bytes as a `quad_topology` artifact, which preserves editable quad faces.

The core commits both artifacts in one database transaction. REST artifact downloads and the Studio download control can retrieve the OBJ. CLI can write it with `--quad-output`; MCP can export it with `export_quad_obj`, restricted to the configured MeshHarbor export directory and refusing to overwrite an existing file.

## Geometry and operation contract

The operation accepts one embedded-buffer GLB containing one closed, static, indexed TRIANGLES primitive. Geometry-only input uses POSITION-only float32 data. The optional textured path additionally requires float32 TEXCOORD_0, optional NORMAL, one PBR material, and embedded PNG base-color and metallic/roughness maps. The textured path allows up to 32 scene nodes only when they form one untransformed parent chain ending at the sole mesh leaf. It rejects other scene hierarchies, animation/skinning, morph targets, extensions, extra meshes or primitives, node transforms, tangents, vertex colors, external images, unsupported material maps, and other attributes. The source asset remains immutable. The sidecar receives geometry only; its original quad topology is stored as a geometry-only OBJ. The GLB preview is triangulated because glTF/GLB has no quad primitive mode.

For supported textured input, MeshHarbor creates a fresh xatlas UV layout for the retopologized geometry and projects the original base-color/metallic-roughness maps through a BVH nearest-surface query. It fails closed if the distance cap is exceeded (2% of the source bounds diagonal), the source-facing-normal dot falls below 0.25, opposing near-equal surfaces make projection ambiguous, or coverage falls below 99.5%. The target atlas is capped at 16 million texels and input images are capped at 16 million pixels. Only those two maps appear in the preview; the OBJ has no UVs or appearance. Passing these gates does not establish perceptual texture quality, thin-feature retention, or correspondence on arbitrary topology.

Input validation requires closed, consistently oriented, edge- and vertex-manifold geometry, rejects duplicate or degenerate faces, and enforces the conservative output budget used by the existing geometry validator. If an otherwise supported textured GLB fails validation, coincident boundary vertices (such as vertices split at an attribute seam) may be rejoined in the solver geometry only. Matching coordinates must be exactly equal; no tolerance, movement, or face removal is applied, and the result must pass the same full validator. The bake still reads the original UVs and textures, and `input_vertices` reports the original GLB vertex count. Geometry-only input is not changed by this fallback. The generated OBJ contains only positions and triangle faces; arbitrary OBJ from a user is never passed directly to QuadriFlow. The wrapper uses fixed arguments and seed, never enables `-sat`, clears the child environment, redirects standard streams to null, uses a private per-job temporary directory, bounds inputs/outputs, and terminates the child after 180 seconds. This is process isolation, not an OS sandbox, and it does not impose a hard memory or CPU quota.

Current limits are 64 MiB input GLB, 100,000 input vertices, 200,000 input triangles, a requested target of 4–50,000 quads (default 10,000), 64 MiB output OBJ, 250,000 output vertices, 500,000 output quads, and 180 seconds. QuadriFlow's target count is approximate.

The output parser enforces UTF-8 and byte/record/count bounds, finite positions in a broad source-bounds envelope, four distinct in-range vertices per triangulable quad, duplicate-face rejection, nonzero face area, closed and consistently oriented edge manifoldness, and connected vertex links. It creates smooth-normal triangle GLB previews and optionally projects the two supported maps under the gates above. It does **not** certify self-intersection freedom, a Hausdorff/surface-error tolerance, thin-feature retention, or high-to-low texture fidelity. The UI and operation report label this limitation; inspect experimental output before production use.

## Check an asset without installing QuadriFlow

In Studio, select a mesh and choose **Check retopology input** even if the backend is unavailable.
From the CLI after importing the GLB, run `meshharbor retopology-preflight <asset-id> --target-quads 1500`.
REST clients can POST `{"target_quads":1500}` to
`/api/v1/assets/<asset-id>/retopology-preflight` under the normal local API authorization.
Use the MCP `preflight_retopology` tool with `asset_id` and optional `target_quads` for the same check.
The default target is 10,000. The response checks source eligibility only:
it does not resolve a native backend, create a job or asset, run the solver,
or guarantee valid results or projected appearance. If the server is already
processing another job, retry the check after the operation ends.

## Verified exact-signed-candidate smoke evidence

The approved candidate run [#43721747855](https://github.com/syi0808/meshharbor/actions/runs/43721747855) used MeshHarbor source commit `9288f601578f8c9b5735a1b255b661f6ebcd1c50` and pinned QuadriFlow source commit `810b7a0967c35b0dc85b4464e3835e26a756c967`. The three platform primary/independent build pairs passed the common byte-equality gate, and the platform signers and fresh candidate-verification jobs passed. The overall run is marked failed because the original Windows native job's SLSA-material recheck failed before execution; do not describe that run as a clean all-platform success.

- Linux x64: [native job](https://github.com/syi0808/meshharbor/actions/runs/43721747855/job/132848222187) executed the exact verified signed candidate against the synthetic geometry-only cube. The MeshHarbor core output-validation test passed, producing the validated quad OBJ and triangle GLB preview. This is geometry validation for that fixture, not real-asset quality evidence.
- macOS ARM64: [native job](https://github.com/syi0808/meshharbor/actions/runs/43721747855/job/132848222186) likewise executed the exact verified signed candidate on the cube and passed the MeshHarbor core output-validation test.
- Windows x64: the original [native job](https://github.com/syi0808/meshharbor/actions/runs/43721747855/job/132848222181) stopped at SLSA-material verification and did not execute the binary. A later read-only [verification run #43731453754](https://github.com/syi0808/meshharbor/actions/runs/43731453754) rechecked the original signed artifact and executed it on the synthetic cube. That Windows follow-up did not run the MeshHarbor core output-validator test; the evidence is native cube execution only.

All three platforms were exercised only with the synthetic geometry-only cube. No textured GLB, rebaked map appearance, or representative user model was tested. The candidate does not make the product backend installable.

The signed candidate Actions ZIPs from run #43721747855 were configured for one-day retention; verification and smoke-evidence artifacts were configured for seven days. These are temporary Actions artifacts, not product packages. The read-only Windows follow-up did not create a new signature or release. See [package preparation and remaining gates](quadriflow-sidecar-packaging.md).

## Upstream and licensing

No QuadriFlow source code is copied into MeshHarbor. The upstream repository was inspected at the pinned commit above. Its README describes the project as MIT, but `LICENSE.txt` contains BSD-style three-clause terms plus an additional grant for publicly available enhancements. Preserve the complete upstream license and copyright notices and obtain license review before distribution.

Other reviewed components declare Boost Software License 1.0 (Lemon), Apache-2.0 (pcg32), and BSD-3-Clause (`parallel_stable_sort.h`, included with TBB). Eigen's Sparse Cholesky path is LGPL; `BUILD_FREE_LICENSE=ON` defines `EIGEN_MPL2_ONLY` and selects Eigen SparseLU. This does not make every dependency MIT. A controlled build should explicitly disable TBB, OpenMP, Gurobi, and Lemon's GLPK, ILOG, COIN, and SoPlex backends. The upstream CMake file adds `-fopenmp` unconditionally for GNU compilers despite the OpenMP option, so every intended platform still needs independent build and packaging verification.

## Packaging work still required

A future distributable build must package a managed QuadriFlow sidecar only after an actual MeshHarbor-issued release bundle passes the in-process verifier, the manifest pins the executable and bundle, independent platform builds are byte-identical, complete notices/SBOM and license review pass, and release approval is recorded. Adjacent executables must remain ignored. Although the candidate was signed and smoke-tested on narrowly scoped fixtures, no approved binary is shipped or installed for any platform, and the full per-crate license/SBOM audit is still outstanding. Do not mark the feature available until those evidence and release gates pass. No model data leaves the local MeshHarbor process.

## Separate triangle-remesher prototype

The optional Rust `remesh = 0.0.5` worker is an experimental isotropic **triangle** remesher. It splits/collapses/flips edges and smooths vertices; it is not QuadriFlow, does not produce quads, does not split assets into separate scene nodes, and is not dispatched by `mesh.retopology`. Its strict geometry-only GLB candidate remains separate. It pins its Cargo dependency graph in `Cargo.lock`, uses a separate child process with private temporary files and a 180-second timeout, and has passed stable/MSRV checks. This is not an OS sandbox or proof of geometric quality.
