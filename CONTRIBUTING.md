# Contributing to MeshHarbor

Use [GitHub Issues](https://github.com/syi0808/meshharbor/issues) for bugs, feature ideas, and benchmark reports. Report vulnerabilities privately as described in the [security policy](SECURITY.md).

## Develop locally

Follow the [quick start](README.md#quick-start) for an embedded Studio build or the [Studio development guide](apps/studio/README.md#local-development) for live UI changes. The core test suite does not need model weights, a model-host account, or a GPU.

From the repository root:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
cd apps/studio
npm ci --ignore-scripts --no-audit --no-fund
npm test
npm run build
```

For changes to rendering, the Studio/API boundary, or packaging, see the [browser regression](apps/studio/README.md#browser-regression) and [embedded Studio smoke tests](docs/embedded-studio-package-smoke.md).

## Open a pull request

- Keep the change focused. Explain the problem, the behavior change, and the checks you ran.
- Add focused tests and update user-facing docs when behavior changes. Use synthetic fixtures or samples that can be redistributed, with attribution where required.
- Keep credentials, local data directories, private logs, model weights, and confidential assets out of commits and attachments.
- For a new dependency or vendored file, record its source, revision, and license, and update the relevant [notice](THIRD-PARTY-NOTICES.md). Do not modify pinned Pixal3D snapshots without updating the [bundle and provenance records](docs/source-provenance.md#runtime-bundle-build-allowlist).
- Submit only material you are authorized to contribute. Project contributions are under the [MIT license](LICENSE); third-party material retains its own terms.

## Share a benchmark

Include the MeshHarbor revision, OS, CPU, GPU/VRAM and system RAM, backend/settings, a shareable input or its provenance, cold and warm timings, peak RAM/VRAM if measured, and an observation about output quality. Distinguish a setup or preflight pass from a completed generation. Sanitize logs and screenshots before attaching them.
