export type AssetKind = 'image' | 'mesh';
export type MeshOperationId = 'mesh.retopology' | 'mesh.cleanup' | 'uv.unwrap' | 'render.optimize' | 'mesh.compress';

export interface Artifact {
  id: string;
  kind: string;
  sha256: string;
  byte_size: number;
  mime_type: string;
}

export interface Asset {
  id: string;
  name: string;
  kind: AssetKind;
  mime_type: string;
  sha256: string;
  byte_size: number;
  source_asset_id?: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
  artifacts: Artifact[];
  model_version?: number;
  model?: {
    id: string;
    name: string;
    generated: boolean;
    current_asset_id: string;
  };
}

export interface AssetPage {
  items: Asset[];
  next_cursor: string | null;
}

export interface ModelVersion {
  asset_id: string;
  number: number;
  parent_number: number | null;
  operation: string | null;
  created_at: string;
}

export interface ModelVersionPage {
  items: ModelVersion[];
  current_asset_id: string;
  next_before: number | null;
}

export interface Operation {
  id: string;
  title: string;
  category: string;
  summary: string;
  implemented: boolean;
}

export type HuggingFaceAccess = 'not_connected' | 'access_denied' | 'verified';
export type HuggingFaceConnectionStatus = 'not_connected' | 'invalid_token' | 'connected';
export type RuntimeBackendId = `${'pixal3d' | 'trellis2' | 'triposg' | 'trellis-text'}-${'mlx' | 'cuda'}`;
export type HuggingFaceBrowserLogin = { id: string } & (
  | { state: 'preparing' | 'signed_in' | 'cancelled' | 'expired' }
  | { state: 'awaiting_authorization'; user_code: string; expires_in: number }
  | { state: 'failed'; message: string }
);

export type GenerationParameter = { id: string; label: string; description?: string; section?: string; tuning?: boolean; single_view_only?: boolean } & (
  | { kind: 'fixed'; default: number }
  | { kind: 'choice'; values: (number | string)[]; default: number | string }
  | { kind: 'integer' | 'number'; min: number; max?: number; default: number }
  | { kind: 'boolean'; default: boolean }
);

export type GenerationInputMode = 'image' | 'text';
export type GenerationInput =
  | { kind: 'image'; asset_id: string }
  | { kind: 'text'; prompt: string };
export type GenerationViewMode = 'single' | 'four' | 'six' | 'custom';
export type PosedViewMode = Exclude<GenerationViewMode, 'single'>;
export type OrbitCamera = { azimuth_degrees: number; elevation_degrees: number; distance: number; fov_degrees: number };
export type PosedGenerationInput = {
  mode: PosedViewMode;
  references: { asset_id: string; camera?: OrbitCamera }[];
};
export type GenerationTuning = Record<string, number | string | boolean>;
export type GenerationParameters = {
  seed?: number;
  resolution?: number;
  texture_size?: number;
  target_faces?: number;
  tuning?: GenerationTuning;
  view_input?: PosedGenerationInput;
};

export interface GenerationProfile {
  id: string;
  parameters: GenerationParameter[];
  input_modes?: GenerationInputMode[];
  view_modes?: GenerationViewMode[];
  max_views?: number;
}

export interface GenerationModel extends GenerationProfile {
  modelId: string;
  label: string;
  multiviewReady: boolean;
  input_modes: GenerationInputMode[];
  view_modes: GenerationViewMode[];
  max_views: number;
}

export interface RuntimeInfo {
  status: string;
  platform: string;
  operations: string[];
  model_catalog: Array<{ id: string; title: string; status: string; backend_ids: string[]; output?: 'textured_glb' | 'geometry_glb' }>;
  quadriflow_backend: {
    backend_id: string;
    status: string;
    available: boolean;
    managed: boolean;
    sidecar_source: string | null;
    install_available: boolean;
    message: string;
    next_step: string;
  };
  model_backends: Array<{
    id: string;
    generation?: GenerationProfile | null;
    multiview_ready?: boolean;
    ready?: boolean;
    status: string;
    accelerator?: string;
    managed_install?: boolean;
    managed_ready?: boolean;
    account_tools_ready?: boolean;
    huggingface?: {
      account: HuggingFaceConnectionStatus | 'checking' | 'unavailable';
      model_access: HuggingFaceAccess | 'checking' | 'unavailable';
    };
    host_supported?: boolean;
    validation_level?: string;
    missing?: string[];
    installable: boolean;
    weights_included: boolean;
    downloads_enabled?: boolean;
    environment_ready?: boolean;
    model_files_ready?: boolean;
    models_downloaded?: boolean;
    naf_conversion_required?: boolean;
    conversion_enabled?: boolean;
    runtime_root?: string;
    model_paths?: string[];
    python?: string;
    gated_model_access_required?: boolean;
    license_notice?: string;
    source?: { repository: string; revision: string };
    pixal3d_cache?: {
      local_checkpoints: number;
      reusable_checkpoints: number;
      total_checkpoints: number;
    };
    local_files?: Partial<{
      token_saved: boolean;
      disconnect_available: boolean;
      remove_models_available: boolean;
      credentials_managed: boolean;
      model_files_managed: boolean;
    }>;
  }>;
  notes: string[];
}

export interface Job {
  id: string;
  operation: string;
  state: 'queued' | 'running' | 'validating' | 'succeeded' | 'failed' | 'cancelling' | 'cancelled';
  input_asset_id?: string | null;
  input_asset_name?: string | null;
  parameters?: { input_mode?: GenerationInputMode; backend_id?: string };
  progress: number;
  result?: {
    asset_id?: string;
    report?: Record<string, unknown>;
    backend_id?: string;
    seed?: number;
    resolution?: number;
    texture_size?: number;
    target_faces?: number;
    elapsed_ms?: number;
  } | null;
  error?: { message?: string } | null;
  created_at: string;
  updated_at: string;
  started_at?: string | null;
  stage?: string | null;
}

export interface JobCommand {
  id: number;
  argv: string[];
  cwd: string | null;
  started_at: string;
  ended_at: string | null;
  outcome: string | null;
  stdout: string;
  stderr: string;
  truncated: boolean;
}

export interface JobStep {
  id: number;
  state: Job['state'];
  stage: string | null;
  at: string;
  message: string | null;
  commands: JobCommand[];
}

export interface SimplificationReport {
  triangles_before: number;
  triangles_after: number;
  target_triangles: number;
  geometric_error: number;
  optimized_primitives: number;
  preserved: string[];
  warnings: string[];
}

export interface ApiErrorBody {
  error?: string;
}



export interface RetopologyPreflight {
  input_vertices: number;
  input_triangles: number;
  solver_vertices: number;
  requested_quads: number;
  coincident_boundary_vertices_joined: boolean;
  appearance_projection_required: boolean;
}
