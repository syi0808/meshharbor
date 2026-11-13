# QuadriFlow sidecar manifest

The manifest file `registry/backends/quadriflow.json` is intentionally empty: it records the pinned candidate source but lists no released artifacts. MeshHarbor does not currently download, install, or execute a packaged QuadriFlow binary. Retopology remains unavailable unless a compatible executable is already placed beside the MeshHarbor executable; the application does not download or install it.

The Rust `quadriflow_manifest` module is a fail-closed, side-effect-free structural validator. It limits the schema to the tested targets (Linux x86_64, macOS arm64, and Windows x86_64), fixes each expected archive name and internal file path, constrains release URLs to MeshHarbor GitHub Releases, pins the upstream repository/commit, and checks size/SHA-256 field syntax. The payload helper can compare caller-provided bytes against a listed size and digest.

It does **not** fetch a release, unpack archives, verify a Sigstore/GitHub attestation, check archive paths, execute the binary, validate an SBOM's completeness, certify license compliance, or approve a release. A syntactically valid manifest is not proof of trust or permission to install. No Actions smoke artifact is included or accepted by this manifest.

The manifest format is documented in [`../schemas/quadriflow-release-manifest.schema.json`](../schemas/quadriflow-release-manifest.schema.json). The offline notice/SBOM evidence helper and its limits are described in [`quadriflow-sidecar-packaging.md`](quadriflow-sidecar-packaging.md).

## Gates before a user-facing installer

1. Build from the pinned QuadriFlow source and fixed native dependencies/toolchains on all supported targets. Current smoke jobs use mutable OS package indexes/Homebrew/vcpkg package state and are evidence runs, not reproducible release builds.
2. Collect and review complete per-platform notices and dependency closure, including the exact package-manager/native libraries. Generate and independently validate a standards-conformant SBOM.
3. Package deterministic, path-safe archives and bind them to provenance for the exact source/workflow commit. Pin build actions and native dependencies to immutable revisions/versions.
4. Publish release artifacts with checksums and a verifiable signing/attestation process. Implement cryptographic verification and safe extraction before any download/install path is enabled.
5. Complete separate release approval, licensing review, and user-facing upgrade/rollback design. Unsigned temporary CI artifacts must never be installed automatically.
