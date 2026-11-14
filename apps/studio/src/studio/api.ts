import type { ApiErrorBody, Artifact, Asset, Job, MeshOperationId, Operation, RuntimeInfo } from './types';

export function quadTopologyArtifact(asset: Asset | null): Artifact | undefined {
  return asset?.artifacts.find((entry) => entry.kind === 'quad_topology' || entry.kind === 'quad_obj');
}

export class MeshHarborApi {
  private readonly dashboardHeaders = { 'X-MeshHarbor-Dashboard': 'local-ui-v1' };

  private async request<T>(path: string, init: RequestInit = {}): Promise<T> {
    const headers = new Headers(init.headers);
    for (const [name, value] of Object.entries(this.dashboardHeaders)) headers.set(name, value);
    if (init.body && !(init.body instanceof Blob) && typeof init.body === 'string') {
      headers.set('Content-Type', 'application/json');
    }
    const response = await fetch(path, { ...init, headers, mode: 'cors' });
    if (!response.ok) {
      let detail = `${response.status} ${response.statusText}`;
      try {
        const payload = (await response.json()) as ApiErrorBody;
        if (payload.error) detail = payload.error;
      } catch {
        // Keep the useful HTTP status when the server did not return JSON.
      }
      throw new Error(detail);
    }
    return (await response.json()) as T;
  }

  async health(): Promise<{ status: string; service: string }> {
    const response = await fetch('/api/v1/health', {
      headers: this.dashboardHeaders,
      mode: 'cors',
    });
    if (!response.ok) throw new Error(`MeshHarbor server returned ${response.status}.`);
    return (await response.json()) as { status: string; service: string };
  }

  runtime(): Promise<RuntimeInfo> {
    return this.request('/api/v1/runtime');
  }

  installQuadriflowBackend(): Promise<{ release_tag: string; executable: string; already_installed: boolean }> {
    return this.request('/api/v1/backends/quadriflow/install', { method: 'POST' });
  }

  setupPixal3dRuntime(): Promise<Job> {
    return this.request('/api/v1/runtime/pixal3d-mlx/setup', { method: 'POST' });
  }

  installPixal3dModels(): Promise<Job> {
    return this.request('/api/v1/runtime/pixal3d-mlx/models', {
      method: 'POST',
      body: JSON.stringify({ confirm_large_download: true, confirm_gated_access: true }),
    });
  }

  setupPixal3dCudaRuntime(): Promise<Job> {
    return this.request('/api/v1/runtime/pixal3d-cuda/setup', { method: 'POST' });
  }

  installPixal3dCudaModels(): Promise<Job> {
    return this.request('/api/v1/runtime/pixal3d-cuda/models', {
      method: 'POST',
      body: JSON.stringify({ confirm_large_download: true, confirm_gated_access: true }),
    });
  }

  convertPixal3dNaf(): Promise<Job> {
    return this.request('/api/v1/runtime/pixal3d-mlx/convert-naf', { method: 'POST' });
  }

  operations(): Promise<Operation[]> {
    return this.request('/api/v1/operations');
  }

  assets(): Promise<Asset[]> {
    return this.request('/api/v1/assets?limit=100');
  }

  importGlb(file: File): Promise<Asset> {
    const filename = encodeURIComponent(file.name);
    return this.request(`/api/v1/assets?filename=${filename}`, {
      method: 'POST',
      headers: { 'Content-Type': 'model/gltf-binary' },
      body: file,
    });
  }

  importImage(file: File): Promise<Asset> {
    const filename = encodeURIComponent(file.name);
    return this.request(`/api/v1/assets?filename=${filename}`, {
      method: 'POST',
      headers: { 'Content-Type': file.type || 'application/octet-stream' },
      body: file,
    });
  }

  artifact(asset: Asset): Promise<Blob> {
    // Quad OBJ is an editing artifact, never an image or GLB preview fallback.
    const candidates = asset.artifacts.filter((entry) => entry.kind !== 'quad_topology' && entry.kind !== 'quad_obj');
    const artifact = candidates.find((entry) => entry.mime_type === asset.mime_type)
      ?? candidates.find((entry) => entry.kind === 'source') ?? candidates[0];
    if (!artifact) throw new Error('This asset has no downloadable artifact.');
    return this.downloadArtifact(artifact);
  }

  downloadArtifact(artifact: Artifact): Promise<Blob> {
    return this.requestBlob(`/api/v1/artifacts/${encodeURIComponent(artifact.id)}`);
  }

  private async requestBlob(path: string): Promise<Blob> {
    const response = await fetch(path, {
      headers: this.dashboardHeaders,
      mode: 'cors',
    });
    if (!response.ok) throw new Error(`Artifact download failed (${response.status}).`);
    return response.blob();
  }

  createSimplifyJob(assetId: string, targetTriangles: number, maxError: number): Promise<Job> {
    return this.request('/api/v1/jobs', {
      method: 'POST',
      body: JSON.stringify({
        operation: 'mesh.simplify',
        input: { asset_id: assetId },
        parameters: { target_triangles: targetTriangles, max_error: maxError },
      }),
    });
  }

  createMeshOperationJob(
    assetId: string,
    operation: MeshOperationId,
    parameters: Record<string, number> = {},
  ): Promise<Job> {
    return this.request('/api/v1/jobs', {
      method: 'POST',
      body: JSON.stringify({
        operation,
        input: { asset_id: assetId },
        parameters,
      }),
    });
  }

  createImageGenerationJob(assetId: string, backendId: string, textureSize: number, targetFaces: number): Promise<Job> {
    return this.request('/api/v1/jobs', {
      method: 'POST',
      body: JSON.stringify({
        operation: 'image.generate',
        input: { asset_id: assetId },
        parameters: { backend_id: backendId, texture_size: textureSize, target_faces: targetFaces },
      }),
    });
  }

  job(jobId: string): Promise<Job> {
    return this.request(`/api/v1/jobs/${encodeURIComponent(jobId)}`);
  }

  async cancelJob(jobId: string): Promise<Job> {
    try {
      return await this.request(`/api/v1/jobs/${encodeURIComponent(jobId)}/cancel`, { method: 'POST' });
    } catch (cancelError) {
      // The worker may claim the queued job before the cancellation reaches the store.
      // Refresh once so Studio can show the authoritative state rather than stale "queued" data.
      try {
        const current = await this.job(jobId);
        if (current.state !== 'queued') return current;
      } catch {
        // Keep the original cancellation error if the status refresh also fails.
      }
      throw cancelError;
    }
  }

  derivedAsset(assetId: string): Promise<Asset> {
    return this.request(`/api/v1/assets/${encodeURIComponent(assetId)}`);
  }
}

