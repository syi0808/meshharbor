import type { ApiErrorBody, Asset, Job, Operation, RuntimeInfo } from './types';

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
    const artifact = asset.artifacts.find((entry) => entry.kind === 'source') ?? asset.artifacts[0];
    if (!artifact) throw new Error('This asset has no downloadable artifact.');
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

  createImageGenerationJob(assetId: string, textureSize: number, targetFaces: number): Promise<Job> {
    return this.request('/api/v1/jobs', {
      method: 'POST',
      body: JSON.stringify({
        operation: 'image.generate',
        input: { asset_id: assetId },
        parameters: { texture_size: textureSize, target_faces: targetFaces },
      }),
    });
  }

  job(jobId: string): Promise<Job> {
    return this.request(`/api/v1/jobs/${encodeURIComponent(jobId)}`);
  }

  cancelJob(jobId: string): Promise<Job> {
    return this.request(`/api/v1/jobs/${encodeURIComponent(jobId)}/cancel`, { method: 'POST' });
  }

  derivedAsset(assetId: string): Promise<Asset> {
    return this.request(`/api/v1/assets/${encodeURIComponent(assetId)}`);
  }
}
