# Building and smoke-testing the embedded Studio binary

The Rust `meshharbor` executable embeds `apps/studio/dist` at compile time when that directory exists. Build the locked Studio bundle before Cargo; otherwise the server serves a local “assets are not built” notice.

## Pinned CI build

The package smoke workflow uses GitHub-hosted Ubuntu 24.04, macOS 15, and Windows Server 2022 runners, Node.js 24.22.3, and Rust 1.88.0. On Windows it downloads the official NASM 3.02 x64 archive, verifies its SHA-256 before extracting it, and supplies `CFLAGS=/std:c11` for AWS-LC's MSVC C11 feature probe.

The workflow then runs:

~~~sh
cd apps/studio
npm ci --ignore-scripts --no-audit --no-fund
npm run build
cd ../..
cargo build --locked --release --package meshharbor --bin meshharbor
node scripts/smoke_embedded_studio.mjs target/release/meshharbor
~~~

On Windows, pass `target/release/meshharbor.exe` to the smoke script. For a source build from PowerShell, put NASM 3.02 on `PATH` and set the compiler flag before invoking Cargo:

~~~powershell
$env:CFLAGS = '/std:c11'
cargo build --locked --release --package meshharbor --bin meshharbor
node scripts/smoke_embedded_studio.mjs target/release/meshharbor.exe
~~~

## What the smoke covers

The script starts the built binary on a temporary loopback port and in a fresh temporary data directory. It verifies the health endpoint, loads the embedded Studio root page, fetches each referenced JavaScript/CSS asset, and then stops the server and removes its temporary data directory. It does not use existing MeshHarbor data, install Python or model packages, or access a model host.

This is a production-binary integration smoke, not an installer or release package. It does not publish a binary artifact, code-sign an executable, exercise browser rendering/visual behavior, or validate image-to-3D inference. The separate QuadriFlow source and candidate workflows are not invoked by this package smoke.
