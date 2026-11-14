# QuadriFlow sidecar release manifest schema

[`../schemas/quadriflow-release-manifest.schema.json`](../schemas/quadriflow-release-manifest.schema.json) describes version 1 of the empty-by-default release manifest. Runtime validation additionally checks unique platform records, exact platform-to-filename/USTAR format/path relationships, and the pinned source/workflow fields.

The schema and manifest are structural/integrity inputs, not a release-approval format. The installer verifies package and payload SHA-256 values against the manifest, but does not cryptographically verify a provenance signature or attestations, certify dependency closure, or establish legal clearance. Keep the manifest empty until those release gates are completed.
