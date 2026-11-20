# Building and smoke-testing the embedded Studio binary

The checkout ships the built Studio client in `apps/studio/dist`. Cargo validates its Vite manifest and embeds the assets into the `meshharbor` executable. A fresh clone needs neither Node.js nor pnpm to build and run Studio.

## Pinned CI build

The package smoke workflow builds the checked-in client on GitHub-hosted Ubuntu 24.04, macOS 15, and Windows Server 2022 runners with Rust 1.88.0. Node.js 24.22.3 runs the smoke script after Cargo; only the Linux browser-test step installs pnpm and Studio test dependencies. On Windows CI downloads the official NASM 3.02 x64 archive, verifies its SHA-256 before extracting it, and supplies `CFLAGS=/std:c11` for AWS-LC's MSVC C11 feature probe.

The workflow then runs:

~~~sh
cargo build --locked --release --package meshharbor --bin meshharbor
node scripts/smoke_embedded_studio.mjs target/release/meshharbor
~~~

On Windows, pass `target/release/meshharbor.exe` to the smoke script. For a source build from PowerShell, put NASM 3.02 on `PATH` and set the compiler flag before invoking Cargo:

~~~powershell
$env:CFLAGS = '/std:c11'
cargo build --locked --release --package meshharbor --bin meshharbor
node scripts/smoke_embedded_studio.mjs target/release/meshharbor.exe
~~~

The separate Studio validation workflow installs from the frozen pnpm lockfile, runs the frontend tests, and verifies that regenerating `apps/studio/dist` produces no Git changes. See the [Studio build guide](../apps/studio/README.md#build) to update the checked-in bundle.

## What the smoke covers

The script starts the built binary on a temporary loopback port and in a fresh temporary data directory. It verifies the health endpoint, loads the embedded Studio root page, fetches each referenced JavaScript/CSS asset, and then stops the server and removes its temporary data directory. It does not use existing MeshHarbor data, install Python or model packages, or access a model host.

The HTTP smoke does not exercise browser rendering. The Linux job also runs a separate [Chromium regression](../apps/studio/README.md#browser-regression) against the same built binary. It uses a generated textured GLB to exercise import, Optimize, Original/Result comparison, and export, checking both rendered texture patches and the downloaded GLB.

These checks do not publish a binary artifact, code-sign an executable, build an installer or release package, validate real-asset PBR fidelity, or exercise image-to-3D inference. This workflow does not invoke the separate QuadriFlow source or candidate workflows.
