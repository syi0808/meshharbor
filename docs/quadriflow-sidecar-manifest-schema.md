# QuadriFlow sidecar release manifest schema

[`../schemas/quadriflow-release-manifest.schema.json`](../schemas/quadriflow-release-manifest.schema.json) documents version 1 of the empty-by-default manifest. Runtime validation is stricter for duplicate platform records and exact platform-to-filename/format/path relationships.

The schema is intentionally not an installer authorization format. It does not establish signing trust, legal clearance, provenance validity, archive safety, or release approval.
