export const MODELS = [
  {
    id: "pixal3d",
    name: "Pixal3D",
    status: "pending",
    installable: false,
    backends: ["cuda", "mlx"],
    sourceRepositories: [
      "https://github.com/syi0808/pixal3d-cuda",
      "https://github.com/syi0808/pixal3d-mlx"
    ],
    licenseReview: {
      status: "pending",
      summary: "The porting code is MIT, but the complete runtime and checkpoint path has not passed license, dependency, and platform review."
    },
    blockers: [
      "The exact NAF checkpoint commercial inference scope is unresolved upstream.",
      "DINOv3 code and gated weights use a separate custom agreement.",
      "The MLX rembg dependency is unpinned and may select the non-commercial BRIA RMBG-2.0 model.",
      "The default MLX hole-fill path uses CGAL components with GPL-3.0-or-later or separate commercial terms.",
      "No end-to-end approved runtime package and platform execution matrix is available yet."
    ]
  }
];

const common = {
  schema_version: 1,
  required_capabilities: [],
  preservation_contract: { source_immutable: true },
  executor: "operation-core",
  progress_events: ["queued", "preparing", "running", "validating", "succeeded", "failed", "cancelling", "cancelled"]
};

export const OPERATIONS = [
  {
    ...common,
    id: "asset.import",
    title: "Import asset",
    ui_visible: true,
    state: "available",
    input_schema: { supported_formats: ["PNG", "JPEG", "GLB"] },
    output_schema: { asset_id: "string", artifact_id: "string", metadata: "object" }
  },
  {
    ...common,
    id: "mesh.simplify",
    title: "Simplification",
    ui_visible: true,
    state: "available_on_this_host",
    required_capabilities: ["static-glb", "native-cxx17"],
    input_schema: { asset_id: "string" },
    parameter_schema: { target_triangles: { type: "integer", minimum: 4, maximum: 10000000, default: 4000 } },
    output_schema: { asset_id: "string", output_artifact_id: "string", report_artifacts: "string[]" },
    preservation_contract: {
      source_immutable: true,
      preserves: ["embedded GLB JSON metadata", "supported material records", "face-corner UVs", "normals"],
      rejects: ["multiple meshes or primitives", "animations", "skins", "morph targets", "extensions", "unsupported vertex attributes"],
      notes: ["The target triangle count is aspirational.", "The source-UV envelope is sampled and does not apply node transforms."]
    }
  },
  {
    ...common,
    id: "image.generate",
    title: "Generate",
    ui_visible: true,
    state: "pending",
    required_capabilities: ["approved-image-to-3d-runtime"],
    input_schema: { image_asset_id: "string", model_id: "string" },
    parameter_schema: {},
    output_schema: { asset_id: "string" },
    blocked_reason: "No model backend has passed full runtime, license, weight, and platform validation."
  },
  ...[
    ["mesh.cleanup", "Mesh Cleanup"],
    ["mesh.separate", "Part Separation"],
    ["mesh.retopology", "Retopology"],
    ["mesh.optimize", "Render Optimization"],
    ["mesh.compress", "Compression"]
  ].map(([id, title]) => ({
    ...common,
    id,
    title,
    ui_visible: true,
    state: "pending",
    input_schema: { asset_id: "string" },
    parameter_schema: {},
    output_schema: { asset_id: "string" },
    blocked_reason: "This operation has not yet passed its implementation and preservation-contract checks."
  }))
];

export function listOperations() {
  return OPERATIONS.map(item => structuredClone(item));
}

export function getOperation(id) {
  return OPERATIONS.find(item => item.id === id) || null;
}

export function listModels() {
  return MODELS.map(model => structuredClone(model));
}

export function listRuntimes() {
  return MODELS.flatMap(model => model.backends.map(backend => ({
    id: `${model.id}-${backend}`,
    model_id: model.id,
    backend,
    status: "pending",
    installed: false,
    installable: false,
    license_status: model.licenseReview.status,
    blockers: [...model.blockers]
  })));
}

export function runtimeDoctor({ workerAvailable, platform = process.platform, arch = process.arch } = {}) {
  return {
    platform,
    architecture: arch,
    native_worker: workerAvailable ? "built" : "not_built",
    mesh_simplification: workerAvailable ? "available_on_this_host" : "build_required",
    image_generation: "pending_license_and_runtime_review",
    runtimes: listRuntimes(),
    warnings: ["This prototype is not a cross-platform release build.", "Model package installation and weight download are disabled until the full execution path is approved."]
  };
}
