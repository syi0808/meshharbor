# MeshHarbor Studio

This is the single-screen Studio client for the local MeshHarbor engine. The first usable workflow is importing a static GLB, viewing it, simplifying it with meshoptimizer, and downloading the derived GLB. Image-to-3D generation, UV re-unwrapping, texture baking, retopology, and other unimplemented operations are shown as unavailable rather than simulated.

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

3. Open the local Vite URL. Paste the temporary bearer token printed by the Rust server. The Vite development server proxies `/api/v1` to the Rust loopback API.

The bearer token is held in application memory only and is not written to browser storage. The Rust API must remain bound to loopback. The browser client sends requests only to its same-origin `/api/v1` path.

## Build

```sh
npm run build
```

Vite writes static files to `apps/studio/dist`. The Rust build script embeds that directory into the MeshHarbor binary, and `meshharbor serve` serves it at `/` on its loopback listener. Rebuild the Rust binary after rebuilding the client. If no `dist` is present at Rust build time, `/` returns a local “Studio assets are not built” notice; it does not start a separate UI server. This is not yet a cross-platform desktop installer.

The preview uses Three.js and the browser's WebGL implementation. No remote fonts, images, or model-viewer scripts are loaded by the Studio page.
