import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  cancelJobNotice,
  canInspectJobHistoryRecord,
  isActiveJob,
  isQueuedJobCancellable,
  jobPollRetryDelay,
  LatestRequestGuard,
  shouldDeferTrackedJobCompletion,
  MeshHarborApi,
  quadTopologyArtifact,
} from './api';
import { ModelPreview } from './ModelPreview';
import type { Artifact, Asset, Job, MeshOperationId, Operation, RuntimeInfo, SimplificationReport } from './types';


type Connection = 'disconnected' | 'connecting' | 'connected';
type StudioMeshOperationId = Exclude<MeshOperationId, 'uv.unwrap'>;

function formatSize(size: number) {
  return size < 1024 * 1024 ? `${(size / 1024).toFixed(0)} KB` : `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

const meshOperationOptions: Array<{ id: StudioMeshOperationId; title: string; description: string }> = [
  {
    id: 'mesh.retopology',
    title: 'Quad retopology (experimental)',
    description: 'QuadriFlow remeshes geometry. Supported embedded base-color/metallic-roughness PNGs are projected only into the triangulated GLB preview; the separate quad OBJ remains geometry-only. Unsupported inputs fail closed.',
  },
  {
    id: 'mesh.cleanup',
    title: 'Cleanup',
    description: 'Remove repeated-index, exact zero-area, and same-winding duplicate faces.',
  },
  {
    id: 'mesh.separate_parts',
    title: 'Separate connected parts',
    description: 'Split disconnected indexed components into primitives within the same mesh; no node separation or welding.',
  },
  {
    id: 'render.optimize',
    title: 'Render optimization',
    description: 'Reorder opaque and masked triangles for cache and overdraw; preserve translucent order.',
  },
  {
    id: 'mesh.compress',
    title: 'Compression',
    description: 'Encode geometry with required KHR_meshopt_compression. The original asset stays editable.',
  },
];

export function App() {
  const [connection, setConnection] = useState<Connection>('disconnected');
  const [showConnect, setShowConnect] = useState(false);
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [operations, setOperations] = useState<Operation[]>([]);
  const [assets, setAssets] = useState<Asset[]>([]);
  const [recentJobs, setRecentJobs] = useState<Job[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [targetTriangles, setTargetTriangles] = useState(4000);
  const [maxError, setMaxError] = useState(0.02);
  const [textureSize, setTextureSize] = useState(1024);
  const [targetFaces, setTargetFaces] = useState(100000);
  const [generationBackend, setGenerationBackend] = useState('auto');
  const [modelDownloadAcknowledged, setModelDownloadAcknowledged] = useState(false);
  const [cudaModelDownloadAcknowledged, setCudaModelDownloadAcknowledged] = useState(false);
  const historySelectionGuard = useRef(new LatestRequestGuard());
  const recentJobsRequestGuard = useRef(new LatestRequestGuard());
  const [job, setJobState] = useState<Job | null>(null);
  const jobRef = useRef<Job | null>(null);
  const setJob = useCallback((next: Job | null) => {
    if (jobRef.current?.id !== next?.id) historySelectionGuard.current.invalidate();
    jobRef.current = next;
    setJobState(next);
  }, []);
  const [report, setReport] = useState<SimplificationReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const downloadInFlight = useRef(false);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [activeView, setActiveView] = useState<'source' | 'result'>('source');
  const fileInputRef = useRef<HTMLInputElement>(null);
  const imageInputRef = useRef<HTMLInputElement>(null);
  const api = useMemo(() => new MeshHarborApi(), []);
  const selectedAsset = assets.find((asset) => asset.id === selectedId) ?? null;
  const quadArtifact = quadTopologyArtifact(selectedAsset);
  const retopologyAvailable = operations.find((operation) => operation.id === 'mesh.retopology')?.implemented === true;
  const quadriflowBackend = runtime?.quadriflow_backend;
  const [installingBackend, setInstallingBackend] = useState(false);
  const isRunning = isActiveJob(job);
  const backendStatus = (id: string) => runtime?.model_backends.find((backend) => backend.id === id)?.status.replaceAll('_', ' ') ?? 'Connect engine';
  const mlxBackend = runtime?.model_backends.find((backend) => backend.id === 'pixal3d-mlx');
  const cudaBackend = runtime?.model_backends.find((backend) => backend.id === 'pixal3d-cuda');
  const automaticBackend = runtime?.platform === 'linux' ? cudaBackend : mlxBackend;
  const selectedGenerationBackend = generationBackend === 'auto'
    ? automaticBackend
    : runtime?.model_backends.find((backend) => backend.id === generationBackend);

  const refreshAssets = useCallback(async (client: MeshHarborApi) => {
    const items = await client.assets();
    setAssets(items);
    setSelectedId((current) => current && items.some((item) => item.id === current) ? current : items.find((item) => item.kind === 'mesh')?.id ?? null);
  }, []);

  function invalidateHistoryRequests() {
    historySelectionGuard.current.invalidate();
    recentJobsRequestGuard.current.invalidate();
  }

  async function refreshRecentJobs() {
    const requestId = recentJobsRequestGuard.current.begin();
    try {
      const jobs = await api.jobs(20);
      if (recentJobsRequestGuard.current.isCurrent(requestId)) setRecentJobs(jobs);
    } catch (cause) {
      if (!recentJobsRequestGuard.current.isCurrent(requestId)) return;
      setError(cause instanceof Error ? cause.message : 'Could not refresh recent job records.');
    }
  }

  async function openRecentJob(jobId: string) {
    const requestId = historySelectionGuard.current.begin();
    setError(null);
    setNotice(null);
    try {
      const latest = await api.job(jobId);
      if (!historySelectionGuard.current.isCurrent(requestId)) return;
      if (shouldDeferTrackedJobCompletion(jobRef.current, latest)) {
        recentJobsRequestGuard.current.invalidate();
        setRecentJobs((current) => [latest, ...current.filter((record) => record.id !== latest.id)]
          .sort((left, right) => Number(right.updated_at) - Number(left.updated_at)
            || right.id.localeCompare(left.id))
          .slice(0, 20));
        setNotice('The live status monitor will apply this result and refresh runtime readiness. No status tracking was stopped.');
        return;
      }
      if (!canInspectJobHistoryRecord(jobRef.current, latest)) {
        setNotice('Wait for the active job to finish before opening a different saved result. Its progress and cancellation controls will remain attached.');
        return;
      }

      const active = isActiveJob(latest);
      let derived: Asset | null = null;
      if (!active && latest.result?.asset_id) {
        derived = await api.derivedAsset(latest.result.asset_id);
        if (!historySelectionGuard.current.isCurrent(requestId)) return;
        if (!canInspectJobHistoryRecord(jobRef.current, latest)) {
          setNotice('A new job is active. Its progress and cancellation controls remain attached; try viewing saved results after it finishes.');
          return;
        }
      }

      recentJobsRequestGuard.current.invalidate();
      setRecentJobs((current) => [latest, ...current.filter((record) => record.id !== latest.id)]
        .sort((left, right) => Number(right.updated_at) - Number(left.updated_at)
          || right.id.localeCompare(left.id))
        .slice(0, 20));

      if (active) {
        if (jobRef.current?.id === latest.id) setJob(latest);
        if (latest.input_asset_id) {
          setSelectedId(latest.input_asset_id);
          setActiveView('source');
        }
        setNotice(jobRef.current?.id === latest.id
          ? `Refreshing the current ${latest.operation} job. Its state is persisted locally.`
          : `Last saved state: ${latest.state} (${latest.progress}%). This read-only check did not resume work. Refresh to check again. A saved active state may be stale after an unexpected engine exit; stop every REST/MCP process using this data directory and run meshharbor jobs recover-interrupted --confirm-no-running-meshharbor before retrying.`);
        return;
      }

      setJob(latest);
      setReport(latest.operation === 'mesh.simplify' && latest.result?.report
        ? latest.result.report as unknown as SimplificationReport
        : null);
      if (latest.input_asset_id) {
        setSelectedId(latest.input_asset_id);
        setActiveView('source');
      }
      if (derived) {
        setAssets((current) => [derived!, ...current.filter((item) => item.id !== derived!.id)]);
        setSelectedId(derived.id);
        setActiveView('result');
      }
      if (latest.state === 'failed') {
        setError(latest.error?.message ?? 'This saved job failed.');
      } else {
        setNotice(`Loaded the saved ${latest.operation} record. No work was restarted.`);
      }
    } catch (cause) {
      if (!historySelectionGuard.current.isCurrent(requestId)) return;
      setError(cause instanceof Error ? cause.message : 'Could not load this saved job.');
    }
  }

  const installQuadriflowBackend = useCallback(async () => {
    if (!quadriflowBackend?.install_available || installingBackend) return;
    setError(null);
    setNotice(null);
    setInstallingBackend(true);
    try {
      await api.installQuadriflowBackend();
      const [runtimeInfo, operationList] = await Promise.all([api.runtime(), api.operations()]);
      setRuntime(runtimeInfo);
      setOperations(operationList);
      setNotice('QuadriFlow backend installation completed.');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'QuadriFlow backend installation failed.');
    } finally {
      setInstallingBackend(false);
    }
  }, [api, installingBackend, quadriflowBackend?.install_available]);

  const connect = useCallback(async () => {
    setError(null);
    setConnection('connecting');
    try {
      const health = await api.health();
      if (health.service !== 'meshharbor' || health.status !== 'ok') throw new Error('This does not look like a MeshHarbor server.');
      const [runtimeInfo, operationList, assetList, jobList] = await Promise.all([
        api.runtime(), api.operations(), api.assets(), api.jobs(20),
      ]);
      setRuntime(runtimeInfo);
      setOperations(operationList);
      setAssets(assetList);
      setRecentJobs(jobList);
      setSelectedId((current) => current && assetList.some((item) => item.id === current) ? current : assetList.find((item) => item.kind === 'mesh')?.id ?? null);
      setConnection('connected');
      setShowConnect(false);
      setNotice('Connected to your local MeshHarbor engine.');
    } catch (cause) {
      setConnection('disconnected');
      const message = cause instanceof Error ? cause.message : 'Could not connect to MeshHarbor.';
      setError(`Could not reach the local MeshHarbor engine. Start "meshharbor serve", then retry. ${message}`);
    }
  }, [api]);

  useEffect(() => { void connect(); }, [connect]);

  useEffect(() => {
    if (!job || !['queued', 'running', 'validating', 'cancelling'].includes(job.state)) return;
    let stopped = false;
    let timer = 0;
    let consecutiveFailures = 0;
    const poll = async () => {
      try {
        const latest = await api.job(job.id);
        if (stopped) return;
        consecutiveFailures = 0;
        setError((current) => current?.startsWith('Could not check job status.') ? null : current);
        if (latest.state === 'succeeded') {
          if (latest.operation.startsWith('runtime.')) {
            let refreshed: [RuntimeInfo, Operation[]] | null = null;
            try {
              refreshed = await Promise.all([api.runtime(), api.operations()]);
            } catch (cause) {
              if (!stopped) {
                setJob(latest);
                setError(cause instanceof Error ? cause.message : 'Runtime setup finished, but status could not be refreshed.');
              }
              return;
            }
            if (stopped || !refreshed) return;
            const [runtimeInfo, operationList] = refreshed;
            setJob(latest);
            setRuntime(runtimeInfo);
            setOperations(operationList);
            setModelDownloadAcknowledged(false);
            setCudaModelDownloadAcknowledged(false);
            setNotice(latest.operation === 'runtime.pixal3d.setup'
              ? 'The isolated Python environment is ready. Sign into Hugging Face from a local terminal before downloading gated model files.'
              : latest.operation === 'runtime.pixal3d.install_models'
                ? 'Pinned model files are ready. MeshHarbor completed the one-time NAF conversion in an isolated tool environment.'
                : latest.operation === 'runtime.pixal3d.cuda_setup'
                  ? 'The isolated CUDA environment and GPU preflight are ready. Sign into Hugging Face in a local terminal before the model download.'
                  : latest.operation === 'runtime.pixal3d.cuda_install_models'
                    ? 'Pinned CUDA model files are ready. Generation can now run offline on this NVIDIA host.'
                    : 'The NAF checkpoint is converted and the Pixal3D runtime is ready.');
            return;
          }
          const jobReport = latest.result?.report;
          setReport(latest.operation === 'mesh.simplify' && jobReport
            ? jobReport as unknown as SimplificationReport
            : null);
          let derived: Asset | null = null;
          if (latest.result?.asset_id) {
            derived = await api.derivedAsset(latest.result.asset_id);
          }
          if (stopped) return;
          setJob(latest);
          if (derived) {
            const completedAsset = derived;
            setAssets((current) => [completedAsset, ...current.filter((item) => item.id !== completedAsset.id)]);
            setSelectedId(completedAsset.id);
            setActiveView('result');
          }
          const resultReport = latest.result?.report ?? {};
          if (latest.operation === 'mesh.cleanup') {
            const removed = Math.max(
              0,
              Number(resultReport.triangles_before ?? 0) - Number(resultReport.triangles_after ?? 0),
            );
            setNotice(removed > 0
              ? `Cleanup complete. Removed ${removed.toLocaleString()} invalid or duplicate triangles.`
              : 'Cleanup complete. No removable triangles were found.');
          } else if (latest.operation === 'render.optimize') {
            const before = Number(resultReport.vertex_cache_misses_before ?? 0);
            const after = Number(resultReport.vertex_cache_misses_after ?? 0);
            setNotice(`Render optimization complete. Estimated vertex-cache misses: ${before.toLocaleString()} → ${after.toLocaleString()}.`);
          } else if (latest.operation === 'mesh.compress') {
            const saved = Number(resultReport.bytes_saved ?? 0);
            setNotice(`Compression complete. Saved ${formatSize(saved)}. This GLB requires KHR_meshopt_compression support; the original remains unchanged.`);
          } else if (latest.operation === 'mesh.separate_parts') {
            const parts = Number(resultReport.connected_parts ?? 0);
            setNotice(`Part separation complete. Found ${parts.toLocaleString()} connected component${parts === 1 ? '' : 's'}, emitted as primitives in the same mesh.`);
          } else if (latest.operation === 'mesh.retopology') {
            const projection = resultReport.appearance_projection;
            const coverage = projection && typeof projection === 'object'
              ? Number((projection as Record<string, unknown>).projection_coverage)
              : Number.NaN;
            const projectionNotice = Number.isFinite(coverage)
              ? ` Supported base-color and metallic-roughness maps passed ${(coverage * 100).toFixed(1)}% projection coverage in the preview.`
              : ' No appearance maps were projected.';
            setNotice(`Experimental retopology complete.${projectionNotice} The GLB preview is triangulated; the separate quad OBJ contains geometry only.`);
          } else if (latest.operation === 'uv.unwrap') {
            const width = Number(resultReport.atlas_width ?? 0);
            const height = Number(resultReport.atlas_height ?? 0);
            const size = width && height ? ` at ${width}×${height}px` : '';
            setNotice(`Atlas UV unwrap and same-mesh texture rebake complete${size}. This is not a high-to-low bake; the source remains unchanged.`);
          } else {
            setNotice(latest.operation === 'image.generate'
              ? 'Image-to-3D generation complete. Your source image is unchanged.'
              : 'Mesh simplification complete. Your original file is unchanged.');
          }
          return;
        }
        if (latest.state === 'failed') {
          let refreshed: [RuntimeInfo, Operation[]] | null = null;
          if (latest.operation.startsWith('runtime.')) {
            try {
              refreshed = await Promise.all([api.runtime(), api.operations()]);
            } catch {
              // The job failure remains the primary actionable result.
            }
          }
          if (stopped) return;
          setJob(latest);
          if (refreshed) {
            setRuntime(refreshed[0]);
            setOperations(refreshed[1]);
          }
          setError(latest.error?.message ?? (latest.operation === 'image.generate'
            ? 'This image generation job failed.'
            : latest.operation === 'mesh.simplify'
              ? 'This simplification job failed.'
              : 'This mesh processing job failed.'));
          return;
        }
        if (latest.state === 'cancelled') {
          setJob(latest);
          setNotice('The queued job was cancelled.');
          return;
        }
        setJob(latest);
        timer = window.setTimeout(poll, 700);
      } catch (cause) {
        if (!stopped) {
          consecutiveFailures += 1;
          const delay = jobPollRetryDelay(consecutiveFailures);
          const detail = cause instanceof Error ? cause.message : 'The local engine could not be reached.';
          setError(`Could not check job status. ${detail} Retrying in ${Math.ceil(delay / 1000)}s.`);
          timer = window.setTimeout(poll, delay);
        }
      }
    };
    timer = window.setTimeout(poll, 300);
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [api, job?.id, job?.state]);

  useEffect(() => {
    if (!job) return;
    recentJobsRequestGuard.current.invalidate();
    setRecentJobs((current) => {
      const next = [job, ...current.filter((record) => record.id !== job.id)];
      next.sort((left, right) => Number(right.updated_at) - Number(left.updated_at)
        || right.id.localeCompare(left.id));
      return next.slice(0, 20);
    });
  }, [job]);

  useEffect(() => {
    if (!showConnect) return;
    const dismiss = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setShowConnect(false);
    };
    window.addEventListener('keydown', dismiss);
    return () => window.removeEventListener('keydown', dismiss);
  }, [showConnect]);

  const previewAsset = selectedAsset;

  const loadPreview = useCallback((asset: Asset) => {
    if (connection !== 'connected') return Promise.reject(new Error('The local MeshHarbor engine is offline.'));
    return api.artifact(asset);
  }, [api, connection]);
  const reportPreviewError = useCallback((message: string) => setError(`Preview: ${message}`), []);

  async function importFiles(files: FileList | File[]) {
    if (connection !== 'connected') {
      setShowConnect(true);
      setError('Start the local MeshHarbor engine, then retry the import.');
      return;
    }
    const file = Array.from(files).find((candidate) => {
      const name = candidate.name.toLowerCase();
      return name.endsWith('.glb') || name.endsWith('.png') || name.endsWith('.jpg') || name.endsWith('.jpeg');
    });
    if (!file) {
      setError('Choose a .glb, .png, or .jpg image file.');
      return;
    }
    if (file.size > 512 * 1024 * 1024) {
      setError('This file is larger than the current 512 MiB import limit.');
      return;
    }
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const isImage = /\.(png|jpe?g)$/i.test(file.name);
      const imported = isImage ? await api.importImage(file) : await api.importGlb(file);
      await refreshAssets(api);
      setSelectedId(imported.id);
      setActiveView('source');
      setReport(null);
      setNotice(`${file.name} imported locally${isImage ? '. Generate a 3D asset from it when a Pixal3D runtime is ready.' : '.'}`);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not import this file.');
    } finally {
      setBusy(false);
    }
  }

  async function simplify() {
    if (!selectedAsset) return;
    invalidateHistoryRequests();
    setError(null);
    setNotice(null);
    setBusy(true);
    setReport(null);
    try {
      const nextJob = await api.createSimplifyJob(selectedAsset.id, Math.max(1, Math.floor(targetTriangles)), maxError);
      setJob(nextJob);
      setActiveView('result');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not start simplification.');
    } finally {
      setBusy(false);
    }
  }

  async function runMeshOperation(operation: StudioMeshOperationId) {
    if (!selectedAsset || selectedAsset.kind !== 'mesh') return;
    invalidateHistoryRequests();
    setError(null);
    setNotice(null);
    setBusy(true);
    setReport(null);
    try {
      const parameters: Record<string, number> = operation === 'render.optimize'
        ? { overdraw_threshold: 1.05 }
        : {};
      const nextJob = await api.createMeshOperationJob(selectedAsset.id, operation, parameters);
      setJob(nextJob);
      setActiveView('result');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not start mesh processing.');
    } finally {
      setBusy(false);
    }
  }

  async function generate() {
    if (!selectedAsset || selectedAsset.kind !== 'image') return;
    invalidateHistoryRequests();
    setError(null);
    setNotice(null);
    setBusy(true);
    setReport(null);
    try {
      const nextJob = await api.createImageGenerationJob(selectedAsset.id, generationBackend, textureSize, Math.max(4, Math.floor(targetFaces)));
      setJob(nextJob);
      setActiveView('result');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not start image generation.');
    } finally {
      setBusy(false);
    }
  }

  async function setupPixal3dRuntime() {
    invalidateHistoryRequests();
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const nextJob = await api.setupPixal3dRuntime();
      setJob(nextJob);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not set up the Pixal3D runtime.');
    } finally {
      setBusy(false);
    }
  }

  async function installPixal3dModels() {
    if (!modelDownloadAcknowledged) return;
    invalidateHistoryRequests();
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const nextJob = await api.installPixal3dModels();
      setJob(nextJob);
      setModelDownloadAcknowledged(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not start the model download.');
    } finally {
      setBusy(false);
    }
  }

  async function convertPixal3dNaf() {
    invalidateHistoryRequests();
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const nextJob = await api.convertPixal3dNaf();
      setJob(nextJob);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not prepare the NAF converter.');
    } finally {
      setBusy(false);
    }
  }

  async function setupPixal3dCudaRuntime() {
    invalidateHistoryRequests();
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const nextJob = await api.setupPixal3dCudaRuntime();
      setJob(nextJob);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not set up the Pixal3D CUDA runtime.');
    } finally {
      setBusy(false);
    }
  }

  async function installPixal3dCudaModels() {
    if (!cudaModelDownloadAcknowledged) return;
    invalidateHistoryRequests();
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const nextJob = await api.installPixal3dCudaModels();
      setJob(nextJob);
      setCudaModelDownloadAcknowledged(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not start the CUDA model download.');
    } finally {
      setBusy(false);
    }
  }

  async function downloadResult(artifact?: Artifact) {
    if (!selectedAsset || downloadInFlight.current || connection !== 'connected') return;
    downloadInFlight.current = true;
    setDownloading(true);
    setError(null);
    try {
      const blob = artifact ? await api.downloadArtifact(artifact) : await api.artifact(selectedAsset);
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = artifact ? `${selectedAsset.name.replace(/\.[^.]+$/, '')}-quads.obj` : selectedAsset.name;
      anchor.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not download this asset.');
    } finally {
      downloadInFlight.current = false;
      setDownloading(false);
    }
  }

  async function cancelJob() {
    if (!isQueuedJobCancellable(job)) return;
    invalidateHistoryRequests();
    try {
      const updated = await api.cancelJob(job.id);
      setJob(updated);
      setNotice(cancelJobNotice(updated));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not cancel this job.');
    }
  }

  function closeNotice() { setNotice(null); }

  const triangleCount = Number(selectedAsset?.metadata.triangles ?? 0);
  const canSimplify = !!selectedAsset && selectedAsset.kind === 'mesh' && !!operations.find((operation) => operation.id === 'mesh.simplify')?.implemented;
  const canGenerate = !!selectedAsset && selectedAsset.kind === 'image'
    && !!operations.find((operation) => operation.id === 'image.generate')?.implemented
    && selectedGenerationBackend?.ready === true;

  return (
    <div className="app-shell" onDragOver={(event) => { event.preventDefault(); setDragging(true); }} onDragLeave={(event) => { if (event.currentTarget === event.target) setDragging(false); }} onDrop={(event) => { event.preventDefault(); setDragging(false); void importFiles(event.dataTransfer.files); }}>
      <header className="topbar">
        <div className="brand-lockup">
          <div className="brand-mark"><span /><span /><span /></div>
          <span className="brand-name">mesh<span>harbor</span></span>
          <span className="brand-divider" />
          <span className="product-label">STUDIO</span>
        </div>
        <div className="topbar-center"><span className="crumb-muted">Workspace</span><span className="crumb-chevron">/</span><span>Mesh processing</span></div>
        <div className="topbar-actions">
          <div className={`engine-status ${connection}`} title={runtime ? `${runtime.platform} · ${runtime.status.replaceAll('_', ' ')}` : undefined}><i />{connection === 'connected' ? 'Local engine ready' : connection === 'connecting' ? 'Connecting…' : 'Engine offline'}</div>
          <button className="avatar-button" aria-label="Connect engine" title="Connect local engine" onClick={() => setShowConnect((value) => !value)}><span className="plug-icon">⌁</span></button>
        </div>
      </header>

      <div className="workspace">
        <aside className="sidebar">
          <div className="sidebar-section-label">WORKSPACE</div>
          <button className="nav-item active"><span className="nav-icon grid-icon" /> Studio</button>
          <button className="nav-item" onClick={() => document.getElementById('asset-library')?.scrollIntoView({ behavior: 'smooth' })}><span className="nav-icon cube-icon" /> Asset library <span className="nav-count">{assets.length}</span></button>
          <div className="sidebar-section-label operations-label">OPERATIONS</div>
          <div className="nav-item selected"><span className="nav-icon simplify-icon">◒</span> Simplify mesh <span className="selected-dot" /></div>
          <button className="nav-item" onClick={() => document.getElementById('mesh-tools')?.scrollIntoView({ behavior: 'smooth' })}><span className="nav-icon">⌘</span> Cleanup &amp; export</button>
          <button className="nav-item" title="Image-to-3D generation" onClick={() => document.getElementById('image-generation')?.scrollIntoView({ behavior: 'smooth' })}><span className="nav-icon sparkle-icon">✳</span> Generate</button>
          <button
            className="nav-item"
            title={retopologyAvailable
              ? 'Experimental QuadriFlow; a verified, manifest-matched managed sidecar is available.'
              : 'Retopology is unavailable until a trusted managed QuadriFlow release is verified; adjacent binaries are ignored.'}
            onClick={() => document.getElementById('mesh-tools')?.scrollIntoView({ behavior: 'smooth' })}
          >
            <span className="nav-icon">⌘</span> Retopology <span className="soon-label">{retopologyAvailable ? 'READY' : 'UNAVAILABLE'}</span>
          </button>
          <div className="sidebar-bottom">
            <div className="local-card">
              <div className="local-card-top"><span className="local-lock">⌑</span><span>PRIVATE BY DESIGN</span></div>
              <p>Your files stay on this computer. Nothing is uploaded to a cloud service.</p>
            </div>
            <div className="version-row"><span>MeshHarbor</span><span>v0.1 · preview</span></div>
          </div>
        </aside>

        <main className="main-content">
          <div className="page-heading">
            <div><div className="eyebrow">YOUR WORKSPACE</div><h1>Studio <span className="heading-dot">.</span></h1><p>Generate from an image or refine a mesh, all locally.</p></div>
            <div className="import-actions">
              <button className="button button-secondary import-button" onClick={() => fileInputRef.current?.click()} disabled={busy}><span className="button-plus">＋</span> Import GLB</button>
              <button className="button button-secondary import-button" onClick={() => imageInputRef.current?.click()} disabled={busy}><span className="button-plus">＋</span> Import image</button>
            </div>
            <input ref={fileInputRef} className="sr-only" type="file" accept=".glb,model/gltf-binary" onChange={(event) => { if (event.currentTarget.files) void importFiles(event.currentTarget.files); event.currentTarget.value = ''; }} />
            <input ref={imageInputRef} className="sr-only" type="file" accept="image/png,image/jpeg,.png,.jpg,.jpeg" onChange={(event) => { if (event.currentTarget.files) void importFiles(event.currentTarget.files); event.currentTarget.value = ''; }} />
          </div>

          {(error || notice) && <div className={`toast ${error ? 'toast-error' : 'toast-success'}`} role={error ? 'alert' : 'status'}><span className="toast-icon">{error ? '!' : '✓'}</span><span>{error ?? notice}</span><button onClick={() => { setError(null); closeNotice(); }} aria-label="Dismiss">×</button></div>}

          <div className="studio-grid">
            <section className="viewer-column">
              <div className="viewer-card">
                <div className="viewer-toolbar">
                  <div className="viewer-title"><span className="tiny-cube" /> {selectedAsset?.kind === 'image' ? 'Image preview' : '3D Preview'} {selectedAsset && <span className="viewer-filename">/ {selectedAsset.name}</span>}</div>
                  <div className="view-toggle" role="tablist" aria-label="Preview version">
                    <button className={activeView === 'source' ? 'active' : ''} onClick={() => { setActiveView('source'); if (selectedAsset?.source_asset_id) setSelectedId(selectedAsset.source_asset_id); }}>Original</button>
                    <button className={activeView === 'result' ? 'active' : ''} onClick={() => { setActiveView('result'); const derivedId = job?.result?.asset_id ?? assets.find((asset) => asset.source_asset_id === selectedAsset?.id)?.id; if (derivedId) setSelectedId(derivedId); }} disabled={!job?.result?.asset_id && !selectedAsset?.source_asset_id && !assets.some((asset) => asset.source_asset_id === selectedAsset?.id)}>Result</button>
                  </div>
                  {quadArtifact && <button className="button button-secondary" title="Original editable quad topology, without textures or UVs" disabled={downloading || connection !== 'connected'} onClick={() => void downloadResult(quadArtifact)}>Download quad OBJ</button>}
                  <button className="icon-button" title="Download selected GLB" aria-label="Download selected GLB" disabled={!selectedAsset || downloading || connection !== 'connected'} onClick={() => void downloadResult()}><DownloadIcon /></button>
                </div>
                <ModelPreview asset={previewAsset} loadModel={loadPreview} onError={reportPreviewError} />
                {selectedAsset && (
                  <div className="asset-meta-strip">
                    <div className="asset-meta-main"><span className="file-badge">{selectedAsset.kind === 'mesh' ? 'GLB' : selectedAsset.mime_type === 'image/png' ? 'PNG' : 'JPG'}</span><div><strong>{selectedAsset.name}</strong><span>{formatSize(selectedAsset.byte_size)} · {selectedAsset.kind === 'mesh' ? `${Number(selectedAsset.metadata.mesh_count ?? 0)} mesh${Number(selectedAsset.metadata.mesh_count ?? 0) === 1 ? '' : 'es'}` : 'Image asset'}</span></div></div>
                    <div className="meta-divider" />
                    {selectedAsset.kind === 'mesh' ? <><div className="meta-stat"><span>TRIANGLES</span><strong>{triangleCount.toLocaleString()}</strong></div><div className="meta-stat"><span>MATERIALS</span><strong>{Number(selectedAsset.metadata.material_count ?? 0)}</strong></div></> : <div className="meta-stat"><span>INPUT</span><strong>{selectedAsset.mime_type === 'image/png' ? 'PNG' : 'JPEG'}</strong></div>}
                    <div className="meta-stat"><span>STATUS</span><strong className="status-ready"><i /> Ready</strong></div>
                  </div>
                )}
              </div>

              {report && <div className="result-card"><div className="result-heading"><div><span className="result-icon">✓</span><strong>Simplification complete</strong></div><span className="result-delta">−{triangleReduction(report)}%</span></div><div className="result-stats"><div><span>Before</span><strong>{report.triangles_before.toLocaleString()}</strong></div><div className="result-arrow">→</div><div><span>After</span><strong>{report.triangles_after.toLocaleString()}</strong></div><div><span>Target</span><strong>{report.target_triangles.toLocaleString()}</strong></div><div><span>Error</span><strong>{report.geometric_error.toFixed(4)}</strong></div></div>{report.warnings.length > 0 && <p className="result-warning">{report.warnings[0]}</p>}</div>}

              <div className="library-section" id="asset-library">
                <div className="section-heading"><div><h2>Recent assets</h2><span>Files stored locally in your workspace</span></div><button className="text-button" onClick={() => void refreshAssets(api)} disabled={connection !== 'connected'}>Refresh <span>↻</span></button></div>
                {assets.length === 0 ? <div className="library-empty">Your imported models will appear here.</div> : <div className="asset-list">{assets.slice(0, 5).map((asset) => <button key={asset.id} className={`asset-row ${asset.id === selectedId ? 'selected' : ''}`} onClick={() => { setSelectedId(asset.id); setActiveView('source'); setReport(null); }}><span className="asset-row-icon"><MiniCube /></span><span className="asset-row-name"><strong>{asset.name}</strong><small>{asset.source_asset_id ? 'Derived result' : 'Original'} · {formatSize(asset.byte_size)}</small></span><span className="asset-row-tris">{Number(asset.metadata.triangles ?? 0).toLocaleString()} <small>tris</small></span>{asset.id === selectedId && <span className="asset-current-dot" />}</button>)}</div>}
              </div>

              <section className="job-history" aria-labelledby="job-history-title">
                <div className="section-heading">
                  <div><h2 id="job-history-title">Recent jobs</h2><span>Saved locally by the engine</span></div>
                  <button className="text-button" onClick={() => void refreshRecentJobs()} disabled={connection !== 'connected'}>Refresh <span>↻</span></button>
                </div>
                <p className="job-history-note">Opening a record only checks its saved state; Studio never resumes or restarts work. After an unexpected engine exit, a saved queued/running state may be stale. Stop all REST/MCP processes for this data directory and run meshharbor jobs recover-interrupted --confirm-no-running-meshharbor before retrying.</p>
                {recentJobs.length === 0
                  ? <div className="job-history-empty">No saved jobs yet.</div>
                  : <div className="job-history-list">{recentJobs.slice(0, 8).map((savedJob) => (
                    <div className={`job-history-row ${savedJob.id === job?.id ? 'selected' : ''}`} key={savedJob.id}>
                      <div className="job-history-copy"><strong>{savedJob.operation}</strong><small>Updated {formatJobTime(savedJob.updated_at)}</small></div>
                      <div className="job-history-status" data-state={savedJob.state}><strong>{savedJob.state}</strong><span>{savedJob.progress}%</span></div>
                      <button className="job-history-open" onClick={() => void openRecentJob(savedJob.id)} disabled={connection !== 'connected' || (isRunning && savedJob.id !== job?.id)}>{['queued', 'running', 'validating', 'cancelling'].includes(savedJob.state) ? 'Check' : 'View'}</button>
                    </div>
                  ))}</div>}
              </section>
            </section>

            <aside className="settings-column">
              <div className="operation-card">
                <div className="operation-header"><div className="operation-symbol"><SimplifyIcon /></div><div><div className="operation-kicker">MESH OPERATION</div><h2>Simplify mesh</h2></div><span className="operation-check">✓</span></div>
                <p className="operation-description">Reduce polygon count while keeping the original file intact and preserving material references.</p>
                <div className="field-label-row"><label htmlFor="triangle-target">Triangle target</label><span className="field-hint">Goal, not a guarantee</span></div>
                <div className="number-input-wrap"><input id="triangle-target" type="number" min="1" max="10000000" step="100" value={targetTriangles} onChange={(event) => setTargetTriangles(Number(event.target.value))} disabled={!selectedAsset || isRunning || busy} /><span>tris</span></div>
                <div className="range-row"><span>More detail</span><input aria-label="Triangle target" type="range" min="100" max={Math.max(1000, triangleCount || 20000)} step="100" value={Math.min(targetTriangles, Math.max(1000, triangleCount || 20000))} onChange={(event) => setTargetTriangles(Number(event.target.value))} disabled={!selectedAsset || isRunning || busy} /><span>Smaller</span></div>
                <div className="field-label-row tolerance-label"><label htmlFor="error-budget">Error tolerance</label><span className="mono-value">{maxError.toFixed(3)}</span></div>
                <input id="error-budget" className="tolerance-range" aria-label="Maximum geometric error" type="range" min="0.001" max="0.08" step="0.001" value={maxError} onChange={(event) => setMaxError(Number(event.target.value))} disabled={!selectedAsset || isRunning || busy} />
                <div className="control-footnote">A lower tolerance may stop above your target to protect shape.</div>
                <div className="preserve-note"><span className="preserve-check">✓</span><span>Original kept unchanged</span><span className="preserve-check">✓</span><span>UVs &amp; materials preserved</span></div>
                {isRunning && job.operation === 'mesh.simplify' ? <div className="job-progress"><div className="job-progress-top"><span><i className="spinner small" /> {job.state === 'queued' ? 'Queued' : job.state === 'validating' ? 'Validating output' : 'Simplifying mesh'}</span><span>{job.progress}%</span></div><div className="progress-track"><span style={{ width: `${job.progress}%` }} /></div>{isQueuedJobCancellable(job) && <button className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</button>}<small>Native simplification can’t be interrupted once it starts.</small></div> : <button className="button button-primary simplify-button" disabled={!canSimplify || busy || connection !== 'connected' || isRunning} onClick={() => void simplify()}>{busy ? <><span className="spinner small" /> Working…</> : <><SimplifyIcon /> Simplify mesh <span className="button-arrow">→</span></>}</button>}
                <section className="generation-card" id="image-generation">
                  <div className="generation-heading"><span className="capability-spark">✳</span><strong>Generate from image</strong></div>
                  <p>Generate a GLB from a local image with the 512 Pixal3D profile. Choose a ready backend; setup and model downloads stay separate opt-in steps.</p>
                  <div className="field-label-row"><label htmlFor="generation-backend">Backend</label><span className="field-hint">Auto selects this host’s backend</span></div>
                  <select id="generation-backend" value={generationBackend} onChange={(event) => setGenerationBackend(event.target.value)} disabled={!selectedAsset || selectedAsset.kind !== 'image' || busy || isRunning}>
                    <option value="auto">Auto ({automaticBackend?.id === 'pixal3d-cuda' ? 'CUDA' : 'MLX'})</option>
                    <option value="pixal3d-mlx">Pixal3D MLX (Apple Silicon)</option>
                    <option value="pixal3d-cuda">Pixal3D CUDA (Linux/NVIDIA)</option>
                  </select>
                  <div className="field-label-row"><label htmlFor="texture-size">Texture size</label><span className="field-hint">Higher uses more memory</span></div>
                  <select id="texture-size" value={textureSize} onChange={(event) => setTextureSize(Number(event.target.value))} disabled={!selectedAsset || selectedAsset.kind !== 'image' || busy || isRunning}>
                    <option value={512}>512</option><option value={1024}>1024</option><option value={2048}>2048</option>
                  </select>
                  <div className="field-label-row generation-face-label"><label htmlFor="target-faces">Soft face target</label><span className="field-hint">Not a hard cap</span></div>
                  <div className="number-input-wrap"><input id="target-faces" type="number" min="4" max="1000000" step="1000" value={targetFaces} onChange={(event) => setTargetFaces(Number(event.target.value))} disabled={!selectedAsset || selectedAsset.kind !== 'image' || busy || isRunning} /><span>faces</span></div>
                  {isRunning && job.operation === 'image.generate' ? <div className="job-progress"><div className="job-progress-top"><span><i className="spinner small" /> {job.state === 'queued' ? 'Queued' : job.state === 'validating' ? 'Validating GLB' : 'Generating 3D'}</span><span>{job.progress}%</span></div><div className="progress-track"><span style={{ width: `${job.progress}%` }} /></div>{isQueuedJobCancellable(job) && <button className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</button>}<small>Running inference cannot be interrupted safely.</small></div> : <button className="button button-primary simplify-button" disabled={!canGenerate || busy || connection !== 'connected' || isRunning} onClick={() => void generate()}>{busy ? <><span className="spinner small" /> Working…</> : <>Generate 3D <span className="button-arrow">→</span></>}</button>}
                  <div className="runtime-setup-panel">
                    <div className="runtime-panel-heading">Pixal3D MLX setup <span>{mlxBackend?.status.replaceAll('_', ' ') ?? 'engine disconnected'}</span></div>
                    {isRunning && job.operation === 'runtime.pixal3d.setup' && <div className="job-progress"><div className="job-progress-top"><span><i className="spinner small" /> {job.state === 'queued' ? 'Waiting for local worker' : 'Preparing isolated Python environment'}</span></div><div className="progress-track"><span /></div>{isQueuedJobCancellable(job) && <button className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</button>}<small>Package downloads may take several minutes. Detailed output appears in the engine terminal.</small></div>}
                    {isRunning && job.operation === 'runtime.pixal3d.install_models' && <div className="job-progress"><div className="job-progress-top"><span><i className="spinner small" /> {job.state === 'queued' ? 'Waiting for local worker' : 'Downloading pinned model files'}</span></div><div className="progress-track"><span /></div>{isQueuedJobCancellable(job) && <button className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</button>}<small>This is a large download (about 24 GB) and may take a while. Per-file transfer output appears in the engine terminal.</small></div>}
                    {isRunning && job.operation === 'runtime.pixal3d.convert_naf' && <div className="job-progress"><div className="job-progress-top"><span><i className="spinner small" /> {job.state === 'queued' ? 'Waiting for local worker' : 'Preparing isolated NAF converter'}</span></div><div className="progress-track"><span /></div>{isQueuedJobCancellable(job) && <button className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</button>}<small>Downloads pinned CPU-only conversion packages into a separate environment, then verifies the converted file.</small></div>}
                    {!isRunning && mlxBackend?.installable && !mlxBackend.environment_ready && <><p>MeshHarbor will create a private Python 3.12 environment and install the pinned backend dependencies. Install <a href="https://docs.astral.sh/uv/getting-started/installation/" target="_blank" rel="noreferrer">uv</a> first if it is not already on PATH.</p><button className="button button-secondary runtime-action" disabled={busy || isRunning || connection !== 'connected'} onClick={() => void setupPixal3dRuntime()}>{busy ? 'Starting…' : 'Set up Python environment'} <span className="button-arrow">→</span></button></>}
                    {!isRunning && mlxBackend?.environment_ready && !mlxBackend.models_downloaded && <><p>Sign in in a separate local terminal with <code>meshharbor runtime hf-login</code>. The token is entered through Hugging Face's masked prompt and stored only in MeshHarbor's isolated cache.</p><p className="runtime-terms">MeshHarbor’s project license has not been selected in this repository, and model-checkpoint terms remain separate. Review the DINOv3 and NAF checkpoint terms and obtain gated access before continuing. This download uses about 24 GB of disk space, then installs an isolated CPU-only NAF conversion environment.</p><label className="runtime-consent"><input type="checkbox" checked={modelDownloadAcknowledged} onChange={(event) => setModelDownloadAcknowledged(event.target.checked)} disabled={busy || isRunning || connection !== 'connected'} /><span>I have reviewed the model terms, have access to gated DINOv3, and am ready to download the files to this computer.</span></label><button className="button button-secondary runtime-action" disabled={!mlxBackend.downloads_enabled || !modelDownloadAcknowledged || busy || isRunning || connection !== 'connected'} onClick={() => void installPixal3dModels()}>{busy ? 'Starting…' : 'Download model files'} <span className="button-arrow">↓</span></button></>}
                    {!isRunning && mlxBackend?.models_downloaded && mlxBackend.naf_conversion_required && <div className="runtime-conversion"><strong>One setup step remains</strong><span>The verified NAF checkpoint needs one-time conversion. MeshHarbor installs the pinned CPU-only converter into a separate environment.</span><button className="button button-secondary runtime-action" disabled={!mlxBackend.conversion_enabled || busy || isRunning || connection !== 'connected'} onClick={() => void convertPixal3dNaf()}>{busy ? 'Starting…' : 'Finish NAF conversion'} <span className="button-arrow">→</span></button></div>}
                    {!isRunning && mlxBackend?.installable === false && <p>Pixal3D MLX setup is supported on Apple Silicon macOS only. CUDA setup is a separate Linux/NVIDIA runtime.</p>}
                    <div className="runtime-panel-heading">Pixal3D CUDA setup <span>{cudaBackend?.status.replaceAll('_', ' ') ?? 'engine disconnected'}</span></div>
                    {isRunning && job.operation === 'runtime.pixal3d.cuda_setup' && <div className="job-progress"><div className="job-progress-top"><span><i className="spinner small" /> {job.state === 'queued' ? 'Waiting for local worker' : 'Installing CUDA Python environment and running GPU checks'}</span></div><div className="progress-track"><span /></div>{isQueuedJobCancellable(job) && <button className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</button>}<small>Uses Python 3.12, PyTorch 2.6.0 with CUDA 12.4, and an app-private per-install lock. Detailed output appears in the engine terminal.</small></div>}
                    {isRunning && job.operation === 'runtime.pixal3d.cuda_install_models' && <div className="job-progress"><div className="job-progress-top"><span><i className="spinner small" /> {job.state === 'queued' ? 'Waiting for local worker' : 'Downloading pinned CUDA model files'}</span></div><div className="progress-track"><span /></div>{isQueuedJobCancellable(job) && <button className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</button>}<small>About 25–30 GB; the DINOv3 access check happens before the checkpoint download.</small></div>}
                    {!isRunning && cudaBackend?.installable && !cudaBackend.environment_ready && <><p>On Linux x86_64 with an NVIDIA GPU, MeshHarbor will create a private Python 3.12 environment, resolve upstream dependency ranges into an app-private lock, install the CUDA 12.4 profile, and run a CUDA/SDPA preflight. This does not download model weights.</p><button className="button button-secondary runtime-action" disabled={busy || isRunning || connection !== 'connected'} onClick={() => void setupPixal3dCudaRuntime()}>{busy ? 'Starting…' : 'Set up CUDA environment'} <span className="button-arrow">→</span></button></>}
                    {!isRunning && cudaBackend?.environment_ready && !cudaBackend.models_downloaded && <><p>Sign in in a separate local terminal with <code>meshharbor runtime cuda-hf-login</code>. Hugging Face's prompt stores credentials only in the isolated CUDA cache.</p><p className="runtime-terms">MeshHarbor’s project license has not been selected in this repository; model/checkpoint terms are separate. Review DINOv3, Pixal3D, and NAF terms and obtain gated DINOv3 access first. The download is about 25–30 GB and needs roughly 40 GiB of free disk space.</p><label className="runtime-consent"><input type="checkbox" checked={cudaModelDownloadAcknowledged} onChange={(event) => setCudaModelDownloadAcknowledged(event.target.checked)} disabled={busy || isRunning || connection !== 'connected'} /><span>I reviewed the model terms, have gated DINOv3 access, and approve downloading the CUDA model files to this computer.</span></label><button className="button button-secondary runtime-action" disabled={!cudaBackend.downloads_enabled || !cudaModelDownloadAcknowledged || busy || isRunning || connection !== 'connected'} onClick={() => void installPixal3dCudaModels()}>{busy ? 'Starting…' : 'Download CUDA model files'} <span className="button-arrow">↓</span></button></>}
                    {!isRunning && cudaBackend?.installable === false && <p>Pixal3D CUDA setup is Linux x86_64/NVIDIA only. Use the Apple MLX backend on supported Macs; the CUDA backend is not available on this host.</p>}
                    {cudaBackend?.license_notice && <p className="runtime-terms">{cudaBackend.license_notice}</p>}
                    {isRunning && job.operation.startsWith('runtime.') && <small className="runtime-serial-note">Runtime installation and generation share one local worker.</small>}
                  </div>
                  {selectedAsset?.kind !== 'image' && <div className="engine-nudge">Import or select a PNG/JPEG image to generate a mesh.</div>}
                  {selectedAsset?.kind === 'image' && !canGenerate && <div className="engine-nudge">{selectedGenerationBackend?.status.replaceAll('_', ' ') ?? 'No supported backend detected'}. Complete that backend’s runtime and model steps before starting inference.</div>}
                  {connection !== 'connected' && <div className="engine-nudge">Start the local engine to generate.</div>}
                </section>
                {connection !== 'connected' && <div className="engine-nudge"><span>●</span> Start the local engine to process models.</div>}
                {selectedAsset && selectedAsset.kind !== 'mesh' && <div className="engine-nudge">Mesh processing requires a GLB mesh file.</div>}
              </div>

              <div className="operation-card mesh-tools-card" id="mesh-tools">
                <div className="operation-header">
                  <div className="operation-symbol"><span className="capability-spark">⌘</span></div>
                  <div><div className="operation-kicker">MESH TOOLS</div><h2>Refine &amp; export</h2></div>
                </div>
                <p className="operation-description">Each operation creates a new asset. Your source file is never overwritten.</p>
                <div className="mesh-tool-list">
                  {meshOperationOptions.map((tool) => {
                    const available = operations.find((operation) => operation.id === tool.id)?.implemented === true;
                    const processing = isRunning && job.operation === tool.id;
                    return (
                      <div className="mesh-tool-item" key={tool.id}>
                        <strong>{tool.title}</strong>
                        <p>{tool.description}</p>
                        {processing ? (
                          <div className="job-progress">
                            <div className="job-progress-top">
                              <span><i className="spinner small" /> {job.state === 'queued' ? 'Queued' : job.state === 'validating' ? 'Validating output' : `Running ${tool.title.toLowerCase()}`}</span>
                              <span>{job.progress}%</span>
                            </div>
                            <div className="progress-track"><span style={{ width: `${job.progress}%` }} /></div>
                            {isQueuedJobCancellable(job) && <button className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</button>}
                            <small>Source asset remains unchanged.</small>
                          </div>
                        ) : (
                          <button
                            className="button button-secondary mesh-tool-button"
                            disabled={!selectedAsset || selectedAsset.kind !== 'mesh' || !available || busy || connection !== 'connected' || isRunning}
                            onClick={() => void runMeshOperation(tool.id)}
                          >
                            Run {tool.title}<span className="button-arrow">→</span>
                          </button>
                        )}
                      </div>
                    );
                  })}
                </div>
                <div className="control-footnote">Compression exports a GLB that requires KHR_meshopt_compression support. The Studio preview includes its decoder. Part separation creates connected-component primitives inside the same mesh; it does not split scene nodes.</div>
              </div>

              <div className="capability-card"><div className="capability-heading"><span className="capability-spark">✳</span><strong>Still in progress</strong></div><p>General high-to-low texture baking remains unavailable. Experimental QuadriFlow retopology is unavailable until a signature-verified, manifest-matched managed sidecar is supported; adjacent binaries are intentionally ignored. Editable quads are preserved in a separate geometry-only OBJ, while supported base-color/metallic-roughness maps are projected only into the triangulated GLB preview. Mesh tools create derived assets and keep originals unchanged. The repository’s project license has not been selected; upstream model terms remain separate.</p><div className="capability-foot"><span>QuadriFlow backend</span><span className="capability-status">{quadriflowBackend?.status.replaceAll('_', ' ') ?? 'Connect engine'}</span></div>{quadriflowBackend && <><p>{quadriflowBackend.message}</p><p>{quadriflowBackend.next_step}</p></>}<button className="button button-primary" onClick={() => void installQuadriflowBackend()} disabled={!quadriflowBackend?.install_available || installingBackend}>{installingBackend ? 'Installing…' : quadriflowBackend?.install_available ? 'Install QuadriFlow' : 'Install unavailable'}</button><div className="capability-foot"><span>CUDA backend</span><span className="capability-status">{backendStatus('pixal3d-cuda')}</span></div><div className="capability-foot"><span>Apple MLX backend</span><span className="capability-status">{backendStatus('pixal3d-mlx')}</span></div></div>
            </aside>
          </div>
        </main>
      </div>

      {dragging && <div className="drop-overlay"><div className="drop-overlay-card"><span className="drop-icon">↓</span><strong>Drop a GLB or source image here</strong><span>Import starts locally on your computer</span></div></div>}

      {showConnect && <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setShowConnect(false); }}><section className="connect-dialog" role="dialog" aria-modal="true" aria-labelledby="connect-title"><button className="dialog-close" onClick={() => setShowConnect(false)} aria-label="Close">×</button><div className="connect-brand"><div className="brand-mark"><span /><span /><span /></div></div><div className="dialog-eyebrow">LOCAL ENGINE</div><h2 id="connect-title">Connect to MeshHarbor</h2><p>The Studio connects directly to the local engine. No access token is needed in this dashboard.</p><div className="connect-steps"><code>meshharbor serve</code><span>→</span><span>Keep the engine running on this computer</span></div>{error && <div className="dialog-error" role="alert">{error}</div>}<button className="button button-primary connect-button" onClick={() => void connect()} disabled={connection === 'connecting'}>{connection === 'connecting' ? <><span className="spinner small" /> Connecting…</> : 'Retry connection'} <span className="button-arrow">→</span></button><div className="dialog-security"><span>⌑</span> The engine accepts dashboard requests only from its local origin.</div></section></div>}
    </div>
  );
}

function formatJobTime(timestamp: string): string {
  const date = new Date(Number(timestamp));
  return Number.isNaN(date.getTime()) ? 'time unavailable' : date.toLocaleString();
}

function triangleReduction(report: SimplificationReport): number {
  if (!report.triangles_before) return 0;
  return Math.max(0, Math.round((1 - report.triangles_after / report.triangles_before) * 100));
}

function DownloadIcon() { return <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 2.8v9.1m0 0 3.3-3.3M10 11.9 6.7 8.6M3.5 13.6v2.2c0 .7.5 1.2 1.2 1.2h10.6c.7 0 1.2-.5 1.2-1.2v-2.2" /></svg>; }
function SimplifyIcon() { return <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m10 2.5 6.2 3.6v7.8L10 17.5l-6.2-3.6V6.1L10 2.5Z" /><path d="m3.8 6.1 6.2 3.7 6.2-3.7M10 9.8v7.7M7.1 4.2l6.1 3.7" /></svg>; }
function MiniCube() { return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m12 3 8 4.5v9L12 21l-8-4.5v-9L12 3Z" /><path d="m4 7.5 8 4.7 8-4.7M12 12.2V21" /></svg>; }

