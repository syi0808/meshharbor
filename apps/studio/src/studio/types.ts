export type AssetKind = 'image' | 'mesh';
export type MeshOperationId = 'mesh.retopology' | 'mesh.cleanup' | 'mesh.separate_parts' | 'uv.unwrap' | 'render.optimize' | 'mesh.compress';

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
}

export interface AssetPage {
  items: Asset[];
  next_cursor: string | null;
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
export type RuntimeBackendId = 'pixal3d-mlx' | 'pixal3d-cuda';
export type HuggingFaceBrowserLogin = { id: string } & (
  | { state: 'preparing' | 'signed_in' | 'cancelled' | 'expired' }
  | { state: 'awaiting_authorization'; user_code: string; expires_in: number }
  | { state: 'failed'; message: string }
);

export type GenerationParameter = { id: string; label: string; description: string; default: number } & (
  | { kind: 'fixed' }
  | { kind: 'choice'; values: number[] }
  | { kind: 'integer'; min: number; max?: number }
);

export interface GenerationProfile {
  id: string;
  label: string;
  parameters: GenerationParameter[];
}

export interface RuntimeInfo {
  status: string;
  platform: string;
  operations: string[];
  model_catalog: Array<{ id: string; title: string; status: string; backend_ids: string[] }>;
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
    ready?: boolean;
    status: string;
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
    python?: string;
    gated_model_access_required?: boolean;
    license_notice?: string;
    local_files?: {
      token_saved: boolean;
      disconnect_available: boolean;
      remove_models_available: boolean;
      credentials_managed: boolean;
      model_files_managed: boolean;
    };
  }>;
  notes: string[];
}

export interface Job {
  id: string;
  operation: string;
  state: 'queued' | 'running' | 'validating' | 'succeeded' | 'failed' | 'cancelling' | 'cancelled';
  input_asset_id?: string | null;
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
