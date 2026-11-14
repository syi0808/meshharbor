# Sigstore verification fixture

`sigstore-conda-attestation.sigstore.json` and
`sigstore-signed-package-2.1.0-hb0f4dca_0.conda` are copied from the
`sigstore-verify` test data in [`sigstore/sigstore-rust` at commit
`fc308a2f0bcef92f10c199c6d75fa72441b329f7`](https://github.com/sigstore/sigstore-rust/tree/fc308a2f0bcef92f10c199c6d75fa72441b329f7/crates/sigstore-verify/test_data/bundles).
The upstream repository is Apache-2.0 licensed. This small, signed public-good
fixture exercises the verifier's cryptographic path only. It is a Conda test
package, not a QuadriFlow release, and its claims must fail MeshHarbor's product
policy.
