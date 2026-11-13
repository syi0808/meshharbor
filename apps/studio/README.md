# MeshHarbor Studio

This is the single-screen Studio client for the local MeshHarbor engine. It imports PNG/JPEG/GLB assets and exposes meshoptimizer simplification, conservative cleanup, connected-part splitting, render-order optimization, KHR_meshopt compression, and conditional QuadriFlow retopology. Retopology is offered only when the local engine reports a compatible `quadriflow` sidecar beside the MeshHarbor executable. The repository does not ship or automatically install that sidecar, so normal builds keep the action disabled. For the supported textured subset, base-color and metallic/roughness maps are projected only into the triangulated GLB preview; the separate quad OBJ is geometry-only. Image-to-3D jobs depend on the separately configured local MLX/CUDA backends.

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

## Operation limits

- `uv.unwrap` is a low-level REST/CLI/MCP operation, not a standalone Studio tool. It accepts one supported static mesh/primitive/material with existing float UVs and embedded PNG base-color and metallic/roughness maps. It creates a new atlas and rebakes those maps from the same mesh's existing UVs. This is not a high-to-low projection or generic texture bake; unsupported maps and layouts are rejected.
- `mesh.retopology` is available only if the adjacent QuadriFlow sidecar is installed. It accepts a closed, static triangle GLB; supported textured inputs require existing float UVs, optional normals, and embedded PNG base-color and metallic/roughness maps. A nearest-surface projection is bounded by source-relative distance, normal orientation, coverage, and opposing-shell ambiguity checks. Only those maps are rebaked into the triangulated GLB preview. The separate OBJ artifact retains editable quad faces but has no UVs or appearance. Unsupported attributes/maps fail closed, and the operation does not certify texture quality, self-intersection freedom, or a measured surface-error tolerance.
- The remaining mesh operations create derived assets and leave source files unchanged. Their accepted GLB layouts and size/count limits are enforced by the Rust engine, not by the UI.

The React/npm bundle and browser workflow have not been verified in this environment; the Studio sources are not evidence of a complete desktop installer or production client build.
