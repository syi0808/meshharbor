# Release checklist

MeshHarbor is currently distributed as a source checkout. A successful embedded Studio smoke or signed QuadriFlow candidate is not an official binary release.

## Source snapshot

- Confirm the first-party [MIT grant](../LICENSE) and preserve all [third-party notices](../THIRD-PARTY-NOTICES.md). Review the exact vendored source scope against [source provenance](source-provenance.md), especially DINOv3-derived code. If the terms or lineage of a component cannot be confirmed, resolve them or exclude the affected files from the distribution, including any Git history distributed with it.
- Scan the intended Git history and release files for secrets, user data, and unlicensed assets. Check absolute paths and identifiers in sample logs and generated evidence as well as tracked files.
- Build and start Studio from a clean checkout using the public instructions. Confirm the [security reporting](../SECURITY.md) link works for a non-maintainer by enabling [private vulnerability reporting](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/configure-for-a-repository).

## Executables and installers

- Audit each target's actual binary and frontend bundle. The current executable embeds both Pixal3D source snapshots even when no model is configured. Ship the applicable license texts, attributions, and a reviewed target-specific SBOM for compiled, vendored, and embedded components. The lockfiles and source notices are inputs, not a complete distribution bundle.
- Run the [embedded Studio smoke](embedded-studio-package-smoke.md) on every advertised platform and the browser regression. Record what passed, test actual package installation where applicable, and document real-asset, hardware, and runtime limits separately from build success.
- Keep weights and checkpoints out of release artifacts. Review separately installed runtime/dependency and checkpoint terms before advertising any image-to-3D configuration as supported or commercially cleared.
- If shipping QuadriFlow, complete its [per-platform dependency and signed sidecar gates](quadriflow-sidecar-packaging.md#reproducibility-and-release-gates). Leave the trusted manifest empty if no approved sidecar is available.
- Verify checksums, signing/provenance, and the contents of the exact archives offered for download. Label CI or smoke-test artifacts as candidates, not installers.
