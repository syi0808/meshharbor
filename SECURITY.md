# Security policy

## Reporting a vulnerability

Do not describe vulnerabilities in public issues or pull requests. Use [GitHub's private vulnerability form](https://github.com/syi0808/meshharbor/security/advisories/new) when it is available. If it is unavailable, open a public issue **only** to ask the maintainer to enable private reporting; do not disclose the affected component, impact, or reproduction details there.

In the private report, include the affected revision, platform, expected and observed behavior, and a minimal shareable reproduction. Remove tokens, private paths, confidential assets, and model weights from attachments. For an upstream-only issue, use that project's disclosure channel and tell us privately if MeshHarbor is affected.

## Supported scope

MeshHarbor is an early source project. Reports against current `main` are welcome; no older release branches are maintained. The API and Studio are designed for one user on loopback, not a shared service or a public reverse proxy. Processes running as the same OS user are inside the trust boundary. Only provision model runtimes, Python packages, and checkpoints from sources you trust and are authorized to use; running them is not OS-sandboxed.
