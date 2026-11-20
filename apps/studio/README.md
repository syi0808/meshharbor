# MeshHarbor Studio

This is the single-screen Studio client for the local MeshHarbor engine. It imports PNG/JPEG/GLB assets and exposes meshoptimizer simplification, conservative cleanup, connected-part splitting, render-order optimization, KHR_meshopt compression, and experimental, currently unavailable QuadriFlow retopology. Retopology is unavailable in the current product build. Studio displays backend status and the next step, and exposes a disabled install request button. The runtime ignores adjacent executables; a managed sidecar can be resolved only after an in-process verifier accepts its release attestation and the executable hash matches the trusted manifest. The in-process verifier is integrated, but the bundled manifest is empty and no approved sidecar is listed, so install/rollback requests fail closed before downloads or backend-directory writes. A signed macOS candidate passed a local archive rehearsal, not the release download or Studio installer. For the supported textured subset, base-color and metallic/roughness maps are projected only into the triangulated GLB preview; the separate quad OBJ is geometry-only. Image-to-3D jobs depend on the separately configured local MLX/CUDA backends.

## Workspace flow

Studio's workbench has tools on the left, the viewport in the center, and the inventory on the right. Inventory starts on **3D models**, where **Open GLB** imports models for inspection. Switch to **Images** to import or browse PNG/JPEG files. **New model** opens a separate image-to-3D form whether or not a model is selected. Choose a reference in that form or attach a displayed image via **Use as reference**. **Settings** in the app bar opens a separate management page. A missing generation runtime does not block opening and optimizing local GLBs.

The right **Inventory** dock opens on a first desktop visit. Browse the thumbnail grid, search by filename, or expand **Filters** within either tab. Use **Inventory** in the header to reopen it. The icon-only left rail always shows one tool panel, initially **New model**. Open **Optimize**, **More tools**, or **Jobs** without selecting an asset. Operations that require a GLB stay disabled until one is selected. Use the dividers or arrow keys to resize the panels; Studio remembers panel sizes and the Inventory's visibility. At 760px or less, the tool panel stacks above the preview and **Inventory** opens a right drawer.

Job starts and outcomes appear as toasts. Work starts immediately if the engine slot is free; only jobs that have to wait are marked **Queued**. Open **Dashboard** to follow active jobs and browse completed, failed, or cancelled records. In **Details**, expand a step to see saved diagnostics, commands, exit status, and available stdout/stderr. Active logs refresh while the dialog is open. Generation output is grouped under the worker command; worker stage changes appear separately in the timeline. Output is limited to the latest 16 KiB per stream before redaction, and jobs created before this feature have no saved execution log. Imports and exports appear while they run, not in persisted history.

Submit multiple generation requests without waiting for the current one to finish. Under **Generation queue**, move waiting jobs earlier or later, or remove them. Other operations keep their places in the local engine queue. Running or interrupted jobs cannot be reordered, and Studio never resumes interrupted jobs automatically.

Studio renders thumbnails from embedded GLB materials and texture maps, or from the imported image. It captures visible cards one at a time and caches up to 128 captures in the current browser's local site storage. The cache survives reload on the same origin, never modifies the asset, and is not part of exported files. A missing capture (for example on a browser without WebGL) leaves the asset selectable.

**Generate model** shows ready model names first, without backend suffixes. Choose a reference image next; the preview appears inside the form without replacing the inspected asset. **Model settings** groups the selected runtime's catalog controls into independently collapsible sections. Output and Sampling open by default, and closing a section does not reset its controls. Studio keeps edits separate per runtime. Geometry resolution is fixed at 512 for the managed Pixal3D profiles. Runtime paths, debug-only flags, and options that would fetch unpinned dependencies are not model controls. Once submitted, a generation appears under **Inventory → 3D models → In progress**. Its card links to the dashboard and is replaced by the actual library asset when successful.

Pixal3D on Apple Silicon uses one reference. The NVIDIA profile also supports 4-view (front/right/back/left), 6-view (adds top/bottom), and custom (2–8 images with per-image azimuth, elevation, distance, and FOV). Presets assume a fixed rig and do not estimate calibration. Use **Set up multiview** if the separate pinned weights are not installed. Studio only starts that transfer after you approve it. Generation itself stays offline.

**Optimize** and **More tools** target the selected GLB. Use **Material**, **Solid**, or **Wireframe** to inspect the model and **Fit view** to restore its framing. On a **Separate connected parts** result, **Parts** uses a different color for each output part. Click a part in the preview or choose it from the dropdown to dim the others, then toggle **Isolate** to hide them completely. Click empty space or choose **All parts** to clear the selection. Dragging still orbits the model. When small fragments share an output primitive, they appear together as **Grouped fragments**. These controls do not modify the GLB.

Opening **Quad retopology (experimental)** checks the selected GLB and quad target automatically. Changing the model or target rechecks the input. The check does not run the solver or queue a job; **Run** also requires an available QuadriFlow backend.

Operation failures appear as toasts. Open **Notifications** in the header to review the last 50 toasts since opening or reloading Studio, even after they close. Use **Details** on a failed job to see its saved error in a dialog. Hover, focus, or tap a **?** icon for extra information.

Each imported or generated model has one Inventory card. Select a queued or running generation card to open its result automatically when it is ready. If you pick another model first, Studio keeps that selection. Edits such as Cleanup, Optimize, and part separation create numbered versions under the card. Use **Version history** below the preview to inspect an older version temporarily or **Restore** it as current without deleting later versions. Editing the current version advances the card; editing a historical version records a branch and leaves the current card unchanged until you restore that branch. Use **Original** and **Result** to compare the job input and output at the same angle, and **Export GLB** to download the version being viewed.

### Workspace review

[The workspace reference review](../../docs/studio-workspace-layout.md) records the measured Meshy and Tripo layouts, feature entry points, and differences in the MeshHarbor responsive design.

The first-visit audit found an icon-only closed library, a centered empty prompt with no preview of the product's work, and setup discoverable only after loading an image. The redesign uses a compact labeled dock, a mesh study made from triangular geometry, and one contextual primary action. It borrows the review method from [unslop by Damir Sokolovsky](https://github.com/heydamir/unslop), not sample UI or artwork.

## UI primitives

Studio uses the official components generated by the shadcn CLI for [preset `bdyyHLKaG`](https://ui.shadcn.com/create?preset=bdyyHLKaG). The registry style is `radix-mira` in `components.json`, with components under `src/components/ui`. The workbench composes Sidebar with keyboard-accessible Resizable panels and uses Sheet drawers on narrow screens. Long inspector forms and operation reports use ScrollArea; generation actions stay outside the scrolling form. Reference and Settings surfaces use Card, Jobs and download history share Item rows, Inventory search uses Input Group, and the viewport mode selector uses Toggle Group. Model removal and local disconnect use Alert Dialog and restore focus when dismissed. Sonner follows Studio's saved theme. Tailwind 4 compiles the styles into the local Vite bundle. `src/index.css` contains the preset theme with MeshHarbor's mint accent, and Inter, Noto Sans, and Phosphor are bundled locally; see [third-party notices](THIRD-PARTY-NOTICES.md).

Use the registry components and their existing variants and sizes. Use utilities on workspace containers for layout rather than overriding component padding, gaps, or height. The shared Button keeps its color transitions but does not move when pressed. The local Slider forwards the accessible label to its single thumb; Progress forwards its numeric value to Radix.

Drag an image from the Inventory onto the single reference or a specific multiview slot to reuse it without uploading again. On touch devices, select the image and use **Use as reference**. The **Original** and **Result** tabs switch the preview; **Export GLB** is below it.

Use **Stop job** in the workbench or Dashboard to cancel queued jobs and running asset jobs. A running mesh job stops at its next safe checkpoint; a running generation stops its worker. Once saving starts, the result cannot be cancelled. Active runtime setup and downloads can only be stopped while queued. A stopped job cannot be resumed.

## Local development

1. Start the Rust API in one terminal:

   ```sh
   cargo run --bin meshharbor -- serve --listen 127.0.0.1:7432
   ```

2. Use Node.js 24 and the pnpm version pinned in `package.json`. In `apps/studio`, install the locked dependencies and start the dev server:

   ```sh
   pnpm install --frozen-lockfile --ignore-scripts
   pnpm dev
   ```

3. Open `http://127.0.0.1:5173`. Studio connects to the local engine automatically without an engine API token. The Vite development server accepts API proxy requests only from that exact local Studio origin and forwards the dashboard marker to the Rust loopback API.

Dashboard API calls use an explicit non-simple header plus an exact same-origin check, not a bearer token. Safe reads may use browser Fetch Metadata when a same-origin browser omits `Origin`; tokenless dashboard writes require an exact `Origin`. The dev server enforces that browser boundary and rejects cross-origin preflights and untrusted Host headers before proxying. For REST compatibility, a valid bearer-authenticated non-browser client may omit `Origin` on writes; when a bearer request does include `Origin`, it must match the loopback `Host`, so cross-origin requests remain rejected. Production and development HTML responses set `Content-Security-Policy: frame-ancestors 'none'` and `X-Frame-Options: DENY` to prevent clickjacking.

The marker is not a secret or local-process authentication: another process running as the same OS user can forge browser headers and is within this single-user local trust boundary. These checks are for hostile websites, cross-origin forms, and other local web origins. They do not defend against malicious code already running as the user. Non-dashboard clients can continue using the bearer token printed by the Rust server. The Rust API and Vite server must remain bound to loopback.

## Settings

Settings has its own URLs and keeps the workbench selection and preview intact. Use **Workbench** to return. Sections appear in a side navigation on desktop and across the top on narrow screens.

- **Models** lists generation models, supported inputs, and backend requirements. Pixal3D has managed Apple MLX and NVIDIA CUDA profiles; setup and weight downloads are separate. TRELLIS Text Large takes a prompt after manual [MLX](../../docs/trellis-text-mlx.md) or [CUDA](../../docs/trellis-text-cuda.md) setup.
- **Connections** manages Hugging Face sign-in for the local runtime, selected automatically for this computer. **Sign in with browser** prepares a pinned official `hf` CLI in a separate app-managed environment the first time and shows a one-time code for the Hugging Face authorization page. **Check sign-in** verifies the account, not model access. **Use an access token instead** remains available. **Disconnect locally** clears the local credentials without revoking the remote token.
- **Downloads & storage** shows model availability, Pixal3D cache hits, and recent setup and download jobs. Use the DINOv3 model link to request access, then return to Studio. File access is checked automatically when viewing this section or returning to the window; you still approve the download separately. **Uninstall model files** clears only this backend’s files, partial transfers, and Xet cache. It keeps the Python environment, credentials, imported or generated assets, and the shared Hugging Face cache. Custom or linked storage locations are not removed by Studio.
- **Preferences** switches between Dark and Light themes. The choice is saved on the current browser origin, and the setting works even when the engine is offline.

The first-time flow is **Models → set up environment → Connections → sign in → Downloads & storage → approve download after the automatic access check**. Re-running Python setup reuses the backend's working private Python 3.12 environment, its dependency cache, and the existing lock. Setup still reconciles the locked dependencies and verifies the environment; it does not install into a global or active shell environment. Model access verification is a metadata-only request. Local credential, model-access, and removal endpoints accept only same-origin Studio requests. MeshHarbor uses matching files from the pinned public Pixal3D snapshot in your regular Hugging Face Hub cache. It hard-links the bytes into the private backend cache on the same volume, or copies them on another volume, without overwriting or removing the originals. Only missing Pixal3D files go to `hf download`, which can resume partial transfers. Gated DINOv3, NAF, and credentials are managed separately.

## Build

You do not need to build the client to use Studio from a checkout. Rebuild it only when changing the frontend:

```sh
pnpm build
```

Vite writes the production assets and their manifest to `apps/studio/dist`. Commit the regenerated bundle alongside frontend changes. The Rust build script embeds the checked-in bundle, and `meshharbor serve` serves it at `/` on its loopback listener. Rebuild the Rust binary to embed newly rebuilt client assets.

When changing bundled dependencies, update the [Studio notices](THIRD-PARTY-NOTICES.md) and [third-party license texts](THIRD-PARTY-LICENSES.txt) for the resulting bundle.

Run `pnpm run test:security` to exercise the Vite middleware's anti-framing headers, exact local-origin guard, and rejection of originless writes/cross-origin preflights/POSTs/untrusted Hosts.

The preview uses Three.js and the browser's WebGL implementation. No remote fonts, images, or model-viewer scripts are loaded by the Studio page.

## Browser regression

From the repository root, build the binary with the checked-in bundle and install the browser test tools:

```sh
cargo build --locked --package meshharbor --bin meshharbor
cd apps/studio
pnpm install --frozen-lockfile --ignore-scripts
pnpm exec playwright install --only-shell chromium
pnpm run test:e2e
```

If testing local UI edits, rebuild the client before building the binary. On Linux, add `--with-deps` to the Chromium install command if system libraries are missing. Set `MESHHARBOR_BINARY` to test a different embedded build. The test starts and stops a loopback engine with a temporary data directory; no model setup or account is needed.

Headless Chromium imports a generated two-material GLB via the file picker, runs **Optimize**, switches between **Original** and **Result**, and downloads both. It checks that both material views render the same four color patches and that the result wireframe shows the simplified mesh. The original download must match the input byte for byte, and the result must have fewer triangles with identical material bindings, UVs, normals, and embedded base-color and metallic/roughness PNG bytes. A separate test runs **UV atlas rebake** with a supported single-material GLB and checks that both new maps decode. Browser checks cover the Mira dimensions, icon rail, panel resizing and persistence, long filenames at narrow widths, and a narrow-screen setup flow with a mocked runtime job. They do not fetch packages or weights or cover cross-browser visual QA.

The **Compression** regression verifies that the Studio result and the reimported download render the same four texture patches as the source. The download must be smaller and require `KHR_meshopt_compression`.

### QuadriFlow release acceptance

This test must run against a binary built with the **reviewed, signed production manifest** for the host. It uses a fresh data directory, checks that installation is available but not yet active, then clicks **Install QuadriFlow** using the normal public release downloader. It checks native retopology on geometry-only and UV-seamed textured fixtures, the exported GLB and geometry-only quad OBJ, and a byte-identical original export. It does not substitute a runner, an unsigned package, or a local manifest.

After rebuilding the embedded binary as in the browser regression instructions, run the following from `apps/studio`, substituting the actual approved release tag:

```sh
MESHHARBOR_QUADRIFLOW_RELEASE_TAG=quadriflow-vX.Y.Z pnpm run test:e2e:quadriflow-release
```

The test fails at the installer status gate when the bundled manifest has no approved release for the host. It is not part of `pnpm run test:e2e`. For evidence requirements and the separate local rehearsal, see [the packaging notes](../../docs/quadriflow-sidecar-packaging.md).

## Persisted job history

Studio loads up to 20 recent job records from the local engine on connection. Opening a completed record can restore its derived asset in the preview. Checking a queued/running record is read-only and never resumes or restarts work; use **Refresh** to fetch the newest saved state. If the engine exited unexpectedly, a saved active state may be stale. Stop every REST/MCP process using the same data directory, then run `meshharbor jobs recover-interrupted --confirm-no-running-meshharbor` before retrying. The job history is not a process-liveness check.

## Operation limits

- **More tools → UV atlas rebake** (`uv.unwrap`) accepts one supported static mesh/primitive/material with existing float UVs and embedded PNG base-color and metallic/roughness maps with linear filtering and repeat wrapping. It creates a new atlas from the existing UVs and reports any zero-area faces omitted from the result. This is not a high-to-low projection or generic texture bake; unsupported maps and layouts are rejected.
- **Separate connected parts** uses shared indices and exact, unambiguous seams without welding vertices. It keeps up to 256 output primitives, grouping excess fragments in the last primitive without dropping triangles. The report distinguishes detected components from output primitives.
- `mesh.retopology` is currently unavailable. Adjacent executables are ignored; only a managed sidecar accepted by an in-process signature/provenance verifier and matched to the trusted manifest may be made ready in a future build. It accepts a closed, static triangle GLB; supported textured inputs require existing float UVs, optional normals, and embedded PNG base-color and metallic/roughness maps. A nearest-surface projection is bounded by source-relative distance, normal orientation, coverage, and opposing-shell ambiguity checks. Only those maps are rebaked into the triangulated GLB preview. The separate OBJ artifact retains editable quad faces but has no UVs or appearance. Unsupported attributes/maps fail closed, and the operation does not certify texture quality, self-intersection freedom, or a measured surface-error tolerance.
- The remaining mesh operations create derived assets and leave source files unchanged. Their accepted GLB layouts and size/count limits are enforced by the Rust engine, not by the UI.

The Studio bundle and local browser workflow can be tested without a model runtime; actual 3D rendering and image-to-3D generation require a suitable WebGL and MLX/CUDA environment, respectively. The Studio sources and local bundle are not a desktop installer.
