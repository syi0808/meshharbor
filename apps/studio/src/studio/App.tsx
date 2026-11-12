import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { MeshHarborApi } from './api';
import { ModelPreview } from './ModelPreview';
import type { Asset, Job, Operation, RuntimeInfo, SimplificationReport } from './types';

type Connection = 'disconnected' | 'connecting' | 'connected';

export function App() {
  const [connection, setConnection] = useState<Connection>('disconnected');
  const [showConnect, setShowConnect] = useState(false);
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [operations, setOperations] = useState<Operation[]>([]);
  const [assets, setAssets] = useState<Asset[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [targetTriangles, setTargetTriangles] = useState(4000);
  const [maxError, setMaxError] = useState(0.02);
  const [textureSize, setTextureSize] = useState(1024);
  const [targetFaces, setTargetFaces] = useState(100000);
  const [job, setJob] = useState<Job | null>(null);
  const [report, setReport] = useState<SimplificationReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [activeView, setActiveView] = useState<'source' | 'result'>('source');
  const fileInputRef = useRef<HTMLInputElement>(null);
  const imageInputRef = useRef<HTMLInputElement>(null);
  const api = useMemo(() => new MeshHarborApi(), []);
  const selectedAsset = assets.find((asset) => asset.id === selectedId) ?? null;
  const isRunning = !!job && ['queued', 'running', 'validating', 'cancelling'].includes(job.state);
  const backendStatus = (id: string) => runtime?.model_backends.find((backend) => backend.id === id)?.status.replaceAll('_', ' ') ?? 'Connect engine';

  const refreshAssets = useCallback(async (client: MeshHarborApi) => {
    const items = await client.assets();
    setAssets(items);
    setSelectedId((current) => current && items.some((item) => item.id === current) ? current : items.find((item) => item.kind === 'mesh')?.id ?? null);
  }, []);

  const connect = useCallback(async () => {
    setError(null);
    setConnection('connecting');
    try {
      const health = await api.health();
      if (health.service !== 'meshharbor' || health.status !== 'ok') throw new Error('This does not look like a MeshHarbor server.');
      const [runtimeInfo, operationList, assetList] = await Promise.all([
        api.runtime(), api.operations(), api.assets(),
      ]);
      setRuntime(runtimeInfo);
      setOperations(operationList);
      setAssets(assetList);
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
    const poll = async () => {
      try {
        const latest = await api.job(job.id);
        if (stopped) return;
        setJob(latest);
        if (latest.state === 'succeeded') {
          setReport(latest.result?.report ?? null);
          if (latest.result?.asset_id) {
            const derived = await api.derivedAsset(latest.result.asset_id);
            if (!stopped) {
              setAssets((current) => [derived, ...current.filter((item) => item.id !== derived.id)]);
              setSelectedId(derived.id);
              setActiveView('result');
            }
          }
          setNotice(latest.operation === 'image.generate'
            ? 'Image-to-3D generation complete. Your source image is unchanged.'
            : 'Mesh simplification complete. Your original file is unchanged.');
          return;
        }
        if (latest.state === 'failed') {
          setError(latest.error?.message ?? (latest.operation === 'image.generate' ? 'This image generation job failed.' : 'This simplification job failed.'));
          return;
        }
        if (latest.state === 'cancelled') {
          setNotice('The queued job was cancelled.');
          return;
        }
        timer = window.setTimeout(poll, 700);
      } catch (cause) {
        if (!stopped) setError(cause instanceof Error ? cause.message : 'Could not check job status.');
      }
    };
    timer = window.setTimeout(poll, 300);
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [api, job?.id, job?.state]);

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
      setNotice(`${file.name} imported locally${isImage ? '. Generate a 3D asset from it when the MLX runtime is ready.' : '.'}`);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not import this file.');
    } finally {
      setBusy(false);
    }
  }

  async function simplify() {
    if (!selectedAsset) return;
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

  async function generate() {
    if (!selectedAsset || selectedAsset.kind !== 'image') return;
    setError(null);
    setNotice(null);
    setBusy(true);
    setReport(null);
    try {
      const nextJob = await api.createImageGenerationJob(selectedAsset.id, textureSize, Math.max(4, Math.floor(targetFaces)));
      setJob(nextJob);
      setActiveView('result');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not start image generation.');
    } finally {
      setBusy(false);
    }
  }

  async function downloadResult() {
    if (!selectedAsset) return;
    try {
      const blob = await api.artifact(selectedAsset);
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = selectedAsset.name;
      anchor.click();
      URL.revokeObjectURL(url);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not download this asset.');
    }
  }

  async function cancelJob() {
    if (!job || job.state !== 'queued') return;
    try {
      const updated = await api.cancelJob(job.id);
      setJob(updated);
      setNotice(updated.state === 'cancelled' ? 'The queued job was cancelled.' : 'This job has already started and cannot be interrupted safely.');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not cancel this job.');
    }
  }

  function closeNotice() { setNotice(null); }

  const triangleCount = Number(selectedAsset?.metadata.triangles ?? 0);
  const canSimplify = !!selectedAsset && selectedAsset.kind === 'mesh' && !!operations.find((operation) => operation.id === 'mesh.simplify')?.implemented;
  const canGenerate = !!selectedAsset && selectedAsset.kind === 'image' && !!operations.find((operation) => operation.id === 'image.generate')?.implemented;
  const formatSize = (size: number) => size < 1024 * 1024 ? `${(size / 1024).toFixed(0)} KB` : `${(size / (1024 * 1024)).toFixed(1)} MB`;

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
          <button className="nav-item" title="Image-to-3D generation" onClick={() => document.getElementById('image-generation')?.scrollIntoView({ behavior: 'smooth' })}><span className="nav-icon sparkle-icon">✳</span> Generate</button>
          <div className="nav-item unavailable" title="Not implemented in this build"><span className="nav-icon">⌘</span> Retopology <span className="soon-label">SOON</span></div>
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
                  <button className="icon-button" title="Download selected GLB" aria-label="Download selected GLB" disabled={!selectedAsset} onClick={() => void downloadResult()}><DownloadIcon /></button>
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
                {assets.length === 0 ? <div className="library-empty">Your imported models will appear here.</div> : <div className="asset-list">{assets.slice(0, 5).map((asset) => <button key={asset.id} className={`asset-row ${asset.id === selectedId ? 'selected' : ''}`} onClick={() => { setSelectedId(asset.id); setActiveView('source'); setReport(null); }}><span className="asset-row-icon"><MiniCube /></span><span className="asset-row-name"><strong>{asset.name}</strong><small>{asset.source_asset_id ? 'Simplified result' : 'Original'} · {formatSize(asset.byte_size)}</small></span><span className="asset-row-tris">{Number(asset.metadata.triangles ?? 0).toLocaleString()} <small>tris</small></span>{asset.id === selectedId && <span className="asset-current-dot" />}</button>)}</div>}
              </div>
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
                {isRunning && job.operation === 'mesh.simplify' ? <div className="job-progress"><div className="job-progress-top"><span><i className="spinner small" /> {job.state === 'queued' ? 'Queued' : job.state === 'validating' ? 'Validating output' : 'Simplifying mesh'}</span><span>{job.progress}%</span></div><div className="progress-track"><span style={{ width: `${job.progress}%` }} /></div>{job.state === 'queued' && <button className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</button>}<small>Native simplification can’t be interrupted once it starts.</small></div> : <button className="button button-primary simplify-button" disabled={!canSimplify || busy || connection !== 'connected' || isRunning} onClick={() => void simplify()}>{busy ? <><span className="spinner small" /> Working…</> : <><SimplifyIcon /> Simplify mesh <span className="button-arrow">→</span></>}</button>}
                <section className="generation-card" id="image-generation">
                  <div className="generation-heading"><span className="capability-spark">✳</span><strong>Generate from image</strong></div>
                  <p>Run the manually prepared local Pixal3D MLX 512 profile. Checkpoints are not downloaded by MeshHarbor.</p>
                  <div className="field-label-row"><label htmlFor="texture-size">Texture size</label><span className="field-hint">Higher uses more memory</span></div>
                  <select id="texture-size" value={textureSize} onChange={(event) => setTextureSize(Number(event.target.value))} disabled={!selectedAsset || selectedAsset.kind !== 'image' || busy || isRunning}>
                    <option value={512}>512</option><option value={1024}>1024</option><option value={2048}>2048</option>
                  </select>
                  <div className="field-label-row generation-face-label"><label htmlFor="target-faces">Soft face target</label><span className="field-hint">Not a hard cap</span></div>
                  <div className="number-input-wrap"><input id="target-faces" type="number" min="4" max="1000000" step="1000" value={targetFaces} onChange={(event) => setTargetFaces(Number(event.target.value))} disabled={!selectedAsset || selectedAsset.kind !== 'image' || busy || isRunning} /><span>faces</span></div>
                  {isRunning && job.operation === 'image.generate' ? <div className="job-progress"><div className="job-progress-top"><span><i className="spinner small" /> {job.state === 'queued' ? 'Queued' : job.state === 'validating' ? 'Validating GLB' : 'Generating 3D'}</span><span>{job.progress}%</span></div><div className="progress-track"><span style={{ width: `${job.progress}%` }} /></div>{job.state === 'queued' && <button className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</button>}<small>Running inference cannot be interrupted safely.</small></div> : <button className="button button-primary simplify-button" disabled={!canGenerate || busy || connection !== 'connected' || isRunning} onClick={() => void generate()}>{busy ? <><span className="spinner small" /> Working…</> : <>Generate 3D <span className="button-arrow">→</span></>}</button>}
                  {selectedAsset?.kind !== 'image' && <div className="engine-nudge">Import or select a PNG/JPEG image to generate a mesh.</div>}
                  {selectedAsset?.kind === 'image' && !canGenerate && <div className="engine-nudge">{backendStatus('pixal3d-mlx')}. Follow the manual runtime setup before starting inference.</div>}
                  {connection !== 'connected' && <div className="engine-nudge">Start the local engine to generate.</div>}
                </section>
                {connection !== 'connected' && <div className="engine-nudge"><span>●</span> Start the local engine to process models.</div>}
                {selectedAsset && selectedAsset.kind !== 'mesh' && <div className="engine-nudge">Simplification requires a GLB mesh file.</div>}
              </div>

              <div className="capability-card"><div className="capability-heading"><span className="capability-spark">✳</span><strong>More is on the way</strong></div><p>UV unwrapping and texture baking are not available in this build yet. The image-to-3D profile requires a manually prepared local runtime.</p><div className="capability-foot"><span>CUDA backend</span><span className="capability-status">{backendStatus('pixal3d-cuda')}</span></div><div className="capability-foot"><span>Apple MLX backend</span><span className="capability-status">{backendStatus('pixal3d-mlx')}</span></div></div>
            </aside>
          </div>
        </main>
      </div>

      {dragging && <div className="drop-overlay"><div className="drop-overlay-card"><span className="drop-icon">↓</span><strong>Drop a GLB or source image here</strong><span>Import starts locally on your computer</span></div></div>}

      {showConnect && <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setShowConnect(false); }}><section className="connect-dialog" role="dialog" aria-modal="true" aria-labelledby="connect-title"><button className="dialog-close" onClick={() => setShowConnect(false)} aria-label="Close">×</button><div className="connect-brand"><div className="brand-mark"><span /><span /><span /></div></div><div className="dialog-eyebrow">LOCAL ENGINE</div><h2 id="connect-title">Connect to MeshHarbor</h2><p>The Studio connects directly to the local engine. No access token is needed in this dashboard.</p><div className="connect-steps"><code>meshharbor serve</code><span>→</span><span>Keep the engine running on this computer</span></div>{error && <div className="dialog-error" role="alert">{error}</div>}<button className="button button-primary connect-button" onClick={() => void connect()} disabled={connection === 'connecting'}>{connection === 'connecting' ? <><span className="spinner small" /> Connecting…</> : 'Retry connection'} <span className="button-arrow">→</span></button><div className="dialog-security"><span>⌑</span> The engine accepts dashboard requests only from its local origin.</div></section></div>}
    </div>
  );
}

function triangleReduction(report: SimplificationReport): number {
  if (!report.triangles_before) return 0;
  return Math.max(0, Math.round((1 - report.triangles_after / report.triangles_before) * 100));
}

function DownloadIcon() { return <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 2.8v9.1m0 0 3.3-3.3M10 11.9 6.7 8.6M3.5 13.6v2.2c0 .7.5 1.2 1.2 1.2h10.6c.7 0 1.2-.5 1.2-1.2v-2.2" /></svg>; }
function SimplifyIcon() { return <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m10 2.5 6.2 3.6v7.8L10 17.5l-6.2-3.6V6.1L10 2.5Z" /><path d="m3.8 6.1 6.2 3.7 6.2-3.7M10 9.8v7.7M7.1 4.2l6.1 3.7" /></svg>; }
function MiniCube() { return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m12 3 8 4.5v9L12 21l-8-4.5v-9L12 3Z" /><path d="m4 7.5 8 4.7 8-4.7M12 12.2V21" /></svg>; }
