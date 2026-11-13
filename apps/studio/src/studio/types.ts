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

export interface Operation {
  id: string;
  title: string;
  category: string;
  summary: string;
  implemented: boolean;
}

export interface RuntimeInfo {
  status: string;
  platform: string;
  operations: string[];
  model_backends: Array<{
    id: string;
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
  }>;
  notes: string[];
}

export interface Job {
  id: string;
  operation: string;
  state: 'queued' | 'running' | 'validating' | 'succeeded' | 'failed' | 'cancelling' | 'cancelled';
  input_asset_id?: string | null;
  progress: number;
  result?: { asset_id?: string; report?: Record<string, unknown>; backend_id?: string } | null;
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

