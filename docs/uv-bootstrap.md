# Pinned uv bootstrap

When a runtime setup command is explicitly requested, MeshHarbor first uses a valid `MESHHARBOR_UV` override or an already available `uv` on `PATH`. If neither exists, it installs Astral uv 0.13.8 under the runtime data directory at
`tools/uv-0.13.8` (for Pixal3D MLX,
`<app-data>/runtimes/pixal3d-mlx/tools/uv-0.13.8`).

The release is pinned to the immutable official GitHub release [0.13.8](https://github.com/astral-sh/uv/releases/tag/0.13.8). The selected asset is chosen from the running OS, CPU architecture, and Linux GNU/musl ABI. Supported targets are macOS arm64/x86-64, Windows arm64/x86-64, and Linux arm64/x86-64 with GNU or musl. Other targets fail with an actionable error and can use an explicitly configured uv executable.

Before extraction or execution, MeshHarbor downloads the asset over HTTPS from `github.com/astral-sh/uv`, limits it to 32 MiB, and verifies its SHA-256 against a compile-time digest pinned from the official release metadata. No release script, shell command string, moving `latest` URL, or unauthenticated checksum fetched at runtime is used. The binary is staged in the MeshHarbor app data directory, checked with `uv --version`, then atomically moved into its versioned tools directory. A failed or interrupted transfer does not replace an existing executable. Concurrent setup attempts use unique staging directories.

The download requires `curl` to be available on `PATH`. Archive extraction uses system `tar` on Unix-like platforms and PowerShell's `Expand-Archive` on Windows. If any prerequisite is absent, setup returns an error; it does not silently run a second installer path. Users behind a restricted network may need to allow the official GitHub release host. The app never changes the host network policy.

uv itself is downloaded only when a user explicitly starts runtime setup. Python, wheels, and model weights remain separate explicit actions. This bootstrap does not enable CUDA or make the Pixal3D MLX backend available on non-Apple-Silicon systems.
