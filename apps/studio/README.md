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
