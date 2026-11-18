# MeshHarbor Studio

This is the single-screen Studio client for the local MeshHarbor engine. It imports PNG/JPEG/GLB assets and exposes meshoptimizer simplification, conservative cleanup, connected-part splitting, render-order optimization, KHR_meshopt compression, and experimental, currently unavailable QuadriFlow retopology. Retopology is unavailable in the current product build. Studio displays backend status and the next step, and exposes a disabled install request button. The runtime ignores adjacent executables; a managed sidecar can be resolved only after an in-process verifier accepts its release attestation and the executable hash matches the trusted manifest. The in-process verifier is integrated, but the bundled manifest is empty and no approved sidecar is listed, so install/rollback requests fail closed before downloads or backend-directory writes. No MeshHarbor-issued candidate bundle has been verified yet. For the supported textured subset, base-color and metallic/roughness maps are projected only into the triangulated GLB preview; the separate quad OBJ is geometry-only. Image-to-3D jobs depend on the separately configured local MLX/CUDA backends.

## Workspace flow

Studio is one edge-to-edge workbench: the app bar, canvas, and docks share the window without a separate page frame. Start with **Open GLB** or **Create from image** (or drop either file type). Studio shows actions for the selected asset: generation for an image, and optimization or more tools for a GLB. **Runtime** in the app bar opens setup without an asset. It distinguishes the Python environment from the separately approved model download. A missing generation runtime does not block opening and optimizing local GLBs.

The **Assets** dock is open on a first desktop visit, and Studio remembers the last dock choice, including closed. On narrow screens the labeled **Assets** and **Jobs** controls open drawers without changing the saved desktop layout. Mesh controls are next to the preview and open only the selected tool. Use **Material**, **Solid**, or **Wireframe** to inspect the model and **Fit view** to restore its framing. Optimization stores a separate result. Select **Original** and **Result** to compare at the same angle, and use **Export GLB** to download the selected version. Opening a saved job does not restart it.

### Workspace review

[Meshy's quick start](https://docs.meshy.ai/en/webapp/getting-started) combines contextual settings, a persistent model viewer, adjacent assets, and toolbar export. [Tripo's Studio guide](https://www.tripo3d.ai/blog/tripo-studio-tutorial-english) separates creation and inspection tools and gives users model properties and step history alongside the viewport. MeshHarbor uses the same viewport-first structure, but makes local file import, non-destructive revisions, and the offline processing boundary explicit. It does not imply parity with their asset thumbnails, texturing, rigging, segmentation, or cloud generation.

The first-visit audit found an icon-only closed library, a centered empty prompt with no preview of the product's work, and setup discoverable only after loading an image. The redesign uses a compact labeled dock, a mesh study made from triangular geometry, and one contextual primary action. It borrows the review method from [unslop by Damir Sokolovsky](https://github.com/heydamir/unslop), not sample UI or artwork.

## UI primitives

Studio adapts [shadcn/ui preset `bdyyHLKaG`](https://ui.shadcn.com/create?preset=bdyyHLKaG) (Mira, neutral dark surfaces, compact controls, small radius, Inter, Noto Sans headings, and Phosphor icons) in local `src/ui` files. It keeps MeshHarbor's existing mint accent and does not require Tailwind. The Latin fonts are bundled locally; see [third-party notices](THIRD-PARTY-NOTICES.md). Use these components for new buttons, fields, dialogs, feedback, and selection controls; use shell CSS for workbench geometry and the canvas. The runtime dialog and narrow drawers use Radix Dialog, the desktop dock uses Radix Collapsible, and the comparison selector uses Radix Toggle Group.

The right-hand inspector docks on desktop and opens as a modal at widths up to 880px; the Assets and Jobs dock continues to collapse independently. There is one live progress and cancellation surface, inside an open tool drawer or on the workbench, never both. On a completed associated operation, Studio returns to the result for review; Export is emphasized while that result is selected and no editor is open.

## Local development

1. Start the Rust API in one terminal:

   ```sh
   cargo run --bin meshharbor -- serve --listen 127.0.0.1:7432
   ```

2. In `apps/studio`, install the pinned JavaScript dependencies from the official npm registry, then run:

   ```sh
   npm install
   npm run dev
   ```

3. Open `http://127.0.0.1:5173`. Studio connects to the local engine automatically; there is no token prompt. The Vite development server accepts API proxy requests only from that exact local Studio origin and forwards the dashboard marker to the Rust loopback API.

Dashboard API calls use an explicit non-simple header plus an exact same-origin check, not a bearer token. Safe reads may use browser Fetch Metadata when a same-origin browser omits `Origin`; tokenless dashboard writes require an exact `Origin`. The dev server enforces that browser boundary and rejects cross-origin preflights and untrusted Host headers before proxying. For REST compatibility, a valid bearer-authenticated non-browser client may omit `Origin` on writes; when a bearer request does include `Origin`, it must match the loopback `Host`, so cross-origin requests remain rejected. Production and development HTML responses set `Content-Security-Policy: frame-ancestors 'none'` and `X-Frame-Options: DENY` to prevent clickjacking.

The marker is not a secret or local-process authentication: another process running as the same OS user can forge browser headers and is within this single-user local trust boundary. These checks are for hostile websites, cross-origin forms, and other local web origins. They do not defend against malicious code already running as the user. Non-dashboard clients can continue using the bearer token printed by the Rust server. The Rust API and Vite server must remain bound to loopback.

## Build

```sh
npm run build
```

Vite writes static files to `apps/studio/dist`. The Rust build script embeds that directory into the MeshHarbor binary, and `meshharbor serve` serves it at `/` on its loopback listener. Rebuild the Rust binary after rebuilding the client. If no `dist` is present at Rust build time, `/` returns a local “Studio assets are not built” notice; it does not start a separate UI server. This is not yet a cross-platform desktop installer.

Run `npm run test:security` to exercise the Vite middleware's anti-framing headers, exact local-origin guard, and rejection of originless writes/cross-origin preflights/POSTs/untrusted Hosts.

The preview uses Three.js and the browser's WebGL implementation. No remote fonts, images, or model-viewer scripts are loaded by the Studio page.

## Browser regression

From the repository root, build the client before compiling the binary:

```sh
cd apps/studio
npm ci --ignore-scripts --no-audit --no-fund
npm run build
npx --no playwright install --only-shell chromium
cd ../..
cargo build --locked --package meshharbor --bin meshharbor
cd apps/studio
npm run test:e2e
```

On Linux, add `--with-deps` to the Chromium install command if system libraries are missing. Set `MESHHARBOR_BINARY` to test a different embedded build. The test starts and stops a loopback engine with a temporary data directory; no model setup or account is needed.

Headless Chromium imports a generated two-material GLB via the file picker, runs **Optimize**, switches between **Original** and **Result**, and downloads both. It checks that both material views render the same four color patches and that the result wireframe shows the simplified mesh. The original download must match the input byte for byte, and the result must have fewer triangles with identical material bindings, UVs, normals, and embedded base-color and metallic/roughness PNG bytes. Separate browser checks cover dock persistence and the narrow runtime dialog with a mocked setup job, without fetching packages or weights. They do not cover cross-browser or real-asset visual QA.

## Persisted job history

Studio loads up to 20 recent job records from the local engine on connection. Opening a completed record can restore its derived asset in the preview. Checking a queued/running record is read-only and never resumes or restarts work; use **Refresh** to fetch the newest saved state. If the engine exited unexpectedly, a saved active state may be stale. Stop every REST/MCP process using the same data directory, then run `meshharbor jobs recover-interrupted --confirm-no-running-meshharbor` before retrying. The job history is not a process-liveness check.

## Operation limits

- `uv.unwrap` is a low-level REST/CLI/MCP operation, not a standalone Studio tool. It accepts one supported static mesh/primitive/material with existing float UVs and embedded PNG base-color and metallic/roughness maps. It creates a new atlas and rebakes those maps from the same mesh's existing UVs. This is not a high-to-low projection or generic texture bake; unsupported maps and layouts are rejected.
- `mesh.retopology` is currently unavailable. Adjacent executables are ignored; only a managed sidecar accepted by an in-process signature/provenance verifier and matched to the trusted manifest may be made ready in a future build. It accepts a closed, static triangle GLB; supported textured inputs require existing float UVs, optional normals, and embedded PNG base-color and metallic/roughness maps. A nearest-surface projection is bounded by source-relative distance, normal orientation, coverage, and opposing-shell ambiguity checks. Only those maps are rebaked into the triangulated GLB preview. The separate OBJ artifact retains editable quad faces but has no UVs or appearance. Unsupported attributes/maps fail closed, and the operation does not certify texture quality, self-intersection freedom, or a measured surface-error tolerance.
- The remaining mesh operations create derived assets and leave source files unchanged. Their accepted GLB layouts and size/count limits are enforced by the Rust engine, not by the UI.

The Studio bundle and local browser workflow can be tested without a model runtime; actual 3D rendering and image-to-3D generation require a suitable WebGL and MLX/CUDA environment, respectively. The Studio sources and local bundle are not a desktop installer.
