# QuadriFlow sidecar release manifest schema

[`../schemas/quadriflow-release-manifest.schema.json`](../schemas/quadriflow-release-manifest.schema.json) describes version 1 of the empty-by-default release manifest. Runtime validation additionally checks unique platform records, exact platform-to-filename/USTAR format/path relationships, and the pinned source/workflow fields.

The schema and manifest are structural/integrity inputs, not a release-approval format. Before installation, the installer verifies the Sigstore provenance bundle and the pinned SLSA identity, source, workflow, materials, and artifact digest policy. That signature verification does not certify complete dependency closure, legal clearance, or release approval. Keep the manifest empty until those separate release gates are completed.
