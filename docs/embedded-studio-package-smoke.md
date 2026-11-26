# Building and smoke-testing the embedded Studio binary

The checkout ships the built Studio client in `apps/studio/dist`. Cargo validates its Vite manifest and embeds the assets into the `meshharbor` executable. A fresh clone needs neither Node.js nor pnpm to build and run Studio.

## CI build matrix

The [MeshHarbor build and Studio smoke workflow](https://github.com/syi0808/meshharbor/actions/workflows/embedded-studio-package-smoke.yml) builds the default CLI with embedded Studio and checks the unpacked executable on a runner of the matching CPU architecture. It runs for relevant changes on pull requests and pushes to `main`, and can also be run manually.

| Package target | GitHub-hosted runner | Archive |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | Ubuntu 22.04 x64 | `.tar.gz` |
| `aarch64-unknown-linux-gnu` | Ubuntu 22.04 ARM64 | `.tar.gz` |
| `x86_64-unknown-linux-musl` | Ubuntu 24.04 x64, Alpine 3.22 build container | `.tar.gz` |
| `aarch64-unknown-linux-musl` | Ubuntu 24.04 ARM64, Alpine 3.22 build container | `.tar.gz` |
| `x86_64-apple-darwin` | macOS 15 Intel | `.tar.gz` |
| `aarch64-apple-darwin` | macOS 15 Apple Silicon | `.tar.gz` |
| `x86_64-pc-windows-msvc` | Windows Server 2022 x64 | `.zip` |
| `aarch64-pc-windows-msvc` | Windows 11 ARM64 | `.zip` |

Use the GNU build on Linux with compatible glibc and libstdc++ (Ubuntu 22.04 is the build and test baseline). The musl build is for environments without those shared libraries, such as Alpine. CI checks that the musl ELF has no dynamic loader or shared library dependencies and runs it on the native Linux runner. The table lists CI environments, not minimum supported OS versions.

CI uses Rust 1.88.0 and the committed Cargo lockfile. Node.js 24.22.3 runs the smoke script; only the Linux x64 GNU browser regression installs pnpm and Studio test dependencies. Windows jobs set `CFLAGS=/std:c11` for AWS-LC's MSVC C11 probe. The x64 job verifies the SHA-256 of the official NASM 3.02 archive before installing it; ARM64 uses the runner's Clang/Visual Studio toolchain instead.

## Download a CI build

A maintainer can select **MeshHarbor build and Studio smoke → Run workflow** in GitHub Actions and enable **Upload unsigned CI build candidates for download**. Each target uploads `meshharbor-ci-<target>` after its build and startup checks pass, independently of the separate Chromium regression. Download it from the run's **Artifacts** section and open the outer Actions ZIP to get the platform archive and its `.sha256` file. Uploads are opt-in and expire after seven days.

On Linux, verify the archive, extract it, and run the included executable from the extracted directory (example for Linux x64 GNU):

~~~sh
sha256sum -c meshharbor-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf meshharbor-x86_64-unknown-linux-gnu.tar.gz
cd meshharbor-x86_64-unknown-linux-gnu
./meshharbor serve --listen 127.0.0.1:7432
~~~

On macOS, use the matching Darwin archive and replace `sha256sum -c` with `shasum -a 256 -c`. On Windows, use `Get-FileHash -Algorithm SHA256` to compare the ZIP's hash to the companion file, extract the ZIP, then run `.\meshharbor.exe serve --listen 127.0.0.1:7432` from the extracted directory. Open **http://127.0.0.1:7432**. The checksum detects corruption; it is not a signature.

These are unsigned CI candidates with the checked-in notices, not official releases or installers. They do not bundle model weights or the optional linked QuadriFlow solver. Building and starting the app does not establish compatibility for GPU inference backends. See the [release checklist](releasing.md) before redistributing an executable.

## Run the smoke from source

~~~sh
cargo build --locked --release --package meshharbor --bin meshharbor
node scripts/smoke_embedded_studio.mjs target/release/meshharbor
~~~

On Windows, pass `target/release/meshharbor.exe` to the smoke script. For an x64 source build from PowerShell, put NASM 3.02 on `PATH` and set the compiler flag before invoking Cargo:

~~~powershell
$env:CFLAGS = '/std:c11'
cargo build --locked --release --package meshharbor --bin meshharbor
node scripts/smoke_embedded_studio.mjs target/release/meshharbor.exe
~~~

The separate Studio validation workflow installs from the frozen pnpm lockfile, runs the frontend tests, and verifies that regenerating `apps/studio/dist` produces no Git changes. See the [Studio build guide](../apps/studio/README.md#build) to update the checked-in bundle.

## What the smoke covers

The script starts the built binary on a temporary loopback port and in a fresh temporary data directory. It verifies the health endpoint, loads the embedded Studio root page, fetches each referenced JavaScript/CSS asset, and then stops the server and removes its temporary data directory. It does not use existing MeshHarbor data, install Python or model packages, or access a model host.

The HTTP smoke does not exercise browser rendering. The Linux x64 GNU job also runs a separate [Chromium regression](../apps/studio/README.md#browser-regression) against the same built binary. It uses a generated textured GLB to exercise import, Optimize, Original/Result comparison, and export, checking both rendered texture patches and the downloaded GLB.

These checks do not code-sign an executable, build an installer, validate real-asset PBR fidelity, or exercise image-to-3D inference. This workflow does not invoke the separate QuadriFlow source or candidate workflows.
