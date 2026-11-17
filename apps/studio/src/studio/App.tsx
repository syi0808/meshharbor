import { Box, CircleAlert, Download, FileImage, ImagePlus, PanelRightClose, Plug, RotateCw, SlidersHorizontal, Wrench, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  assetComparisonPairForSelection,
  assetComparisonPairFromJob,
  shouldAutoSelectComparisonResult,
  importAssetBatch,
  MAX_ASSET_IMPORT_FILES,
  planAssetImportUiCompletion,
  cancelJobNotice,
  canInspectJobHistoryRecord,
  isActiveJob,
  isQueuedJobCancellable,
  jobReportMatchesAsset,
  jobResultCardModel,
  jobPollRetryDelay,
  LatestRequestGuard,
  preferTrackedTerminalJob,
  shouldDeferTrackedJobCompletion,
  resolveComparisonAsset,
  MeshHarborApi,
  quadTopologyArtifact,
  type AssetComparisonPair,
  type AssetComparisonSide,
  type AssetPageFilters,
  type JobReportCardModel,
} from './api';
import { Alert, AlertDescription } from '../ui/Alert';
import { Badge } from '../ui/Badge';
import { Checkbox } from '../ui/Checkbox';
import { Button } from '../ui/Button';
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle, DialogTrigger } from '../ui/Dialog';
import { Input } from '../ui/Input';
import { Label } from '../ui/Label';
import { NativeSelect } from '../ui/NativeSelect';
import { Progress } from '../ui/Progress';
import { Sheet, SheetClose } from '../ui/Sheet';
import { Slider } from '../ui/Slider';
import { ToggleGroup, ToggleGroupItem } from '../ui/ToggleGroup';
import { ModelPreview } from './ModelPreview';
import { useWorkbenchSidebar, WorkbenchSidebar } from './WorkbenchSidebar';
import { formatVersionSizes, inspectorForAsset, shouldHighlightExport, WORKBENCH_INSPECTOR_SHEET_QUERY, type Inspector } from './workbench-state';
import type { Artifact, Asset, AssetKind, Job, MeshOperationId, Operation, RuntimeInfo } from './types';

type Connection = 'disconnected' | 'connecting' | 'connected';
type StudioMeshOperationId = Exclude<MeshOperationId, 'uv.unwrap'>;
type AssetKindFilter = AssetKind | 'all';
type AssetOriginFilter = 'all' | 'original' | 'derived';
interface AssetBrowserFilters {
  query: string;
  kind: AssetKindFilter;
  origin: AssetOriginFilter;
}

const ASSET_PAGE_SIZE = 50;
const EMPTY_ASSET_BROWSER_FILTERS: AssetBrowserFilters = { query: '', kind: 'all', origin: 'all' };

function toAssetPageFilters(filters: AssetBrowserFilters): AssetPageFilters {
  const query = filters.query.trim();
  return {
    ...(query ? { query } : {}),
    ...(filters.kind === 'all' ? {} : { kind: filters.kind }),
    ...(filters.origin === 'all' ? {} : { derived: filters.origin === 'derived' }),
  };
}

function matchesAssetBrowserFilters(asset: Asset, filters: AssetBrowserFilters): boolean {
  return (!filters.query || asset.name.toLowerCase().includes(filters.query.toLowerCase()))
    && (filters.kind === 'all' || asset.kind === filters.kind)
    && (filters.origin === 'all' || Boolean(asset.source_asset_id) === (filters.origin === 'derived'));
}

function clampAssetSearchQuery(query: string): string {
  let byteLength = 0;
  let bounded = '';
  for (const character of query.trim()) {
    const characterBytes = new TextEncoder().encode(character).byteLength;
    if (byteLength + characterBytes > 256) break;
    bounded += character;
    byteLength += characterBytes;
  }
  return bounded;
}

function formatSize(size: number) {
  return size < 1024 ? `${size} B` : size < 1024 * 1024 ? `${(size / 1024).toFixed(0)} KiB` : `${(size / (1024 * 1024)).toFixed(2)} MiB`;
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
    description: 'Encode geometry with KHR_meshopt_compression. Other viewers need its decoder; Studio includes one.',
  },
];

export function App() {
  const [connection, setConnection] = useState<Connection>('disconnected');
  const [showConnect, setShowConnect] = useState(false);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [operations, setOperations] = useState<Operation[]>([]);
  const [assets, setAssets] = useState<Asset[]>([]);
  const [assetSearchInput, setAssetSearchInput] = useState('');
  const assetSearchInputRef = useRef('');
  const [assetFilters, setAssetFilters] = useState<AssetBrowserFilters>(EMPTY_ASSET_BROWSER_FILTERS);
  const assetFiltersRef = useRef(assetFilters);
  const assetBrowserInteractionRevision = useRef(0);
  const assetSelectionInteractionRevision = useRef(0);
  const [assetNextCursor, setAssetNextCursor] = useState<string | null>(null);
  const [assetPageLoading, setAssetPageLoading] = useState(false);
  const assetPageRequestGuard = useRef(new LatestRequestGuard());
  const [recentJobs, setRecentJobs] = useState<Job[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selectedIdRef = useRef<string | null>(null);
  const [comparisonPair, setComparisonPair] = useState<AssetComparisonPair | null>(null);
  const comparisonPairRef = useRef(comparisonPair);
  const [comparisonLoadingSide, setComparisonLoadingSide] = useState<AssetComparisonSide | null>(null);
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
  const [reportJob, setReportJob] = useState<Job | null>(null);
  const [resolvedPreviewSource, setResolvedPreviewSource] = useState<Asset | null>(null);
  const [busy, setBusy] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const downloadInFlight = useRef(false);
  const [inspector, setInspector] = useState<Inspector | null>(null);
  const inspectorTrigger = useRef<HTMLButtonElement | null>(null);
  const [inspectorIsSheet, setInspectorIsSheet] = useState(() => window.matchMedia(WORKBENCH_INSPECTOR_SHEET_QUERY).matches);
  useEffect(() => {
    const media = window.matchMedia(WORKBENCH_INSPECTOR_SHEET_QUERY);
    const update = () => setInspectorIsSheet(media.matches);
    media.addEventListener('change', update);
    update();
    return () => media.removeEventListener('change', update);
  }, []);
  const sidebar = useWorkbenchSidebar();
  const [workbenchNode, setWorkbenchNode] = useState<HTMLElement | null>(null);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const imageInputRef = useRef<HTMLInputElement>(null);
  const importBatchInFlight = useRef(false);
  const api = useMemo(() => new MeshHarborApi(), []);
  const selectedAsset = assets.find((asset) => asset.id === selectedId) ?? null;
  const reportCard: JobReportCardModel | null = useMemo(() => {
    if (!jobReportMatchesAsset(reportJob, selectedId)) return null;
    return jobResultCardModel(reportJob);
  }, [reportJob, selectedId]);
  const activeJobInputAssetId = job && isActiveJob(job) ? job.input_asset_id ?? null : null;
  const selectedComparisonPair = assetComparisonPairForSelection(
    assets,
    selectedId,
    comparisonPair,
    activeJobInputAssetId,
  );
  const pinnedAssetIds = new Set([
    selectedId,
    comparisonPair?.inputAssetId,
    comparisonPair?.resultAssetId,
    selectedComparisonPair?.inputAssetId,
    selectedComparisonPair?.resultAssetId,
  ].filter((id): id is string => id !== null && id !== undefined));
  const matchingAssets = assets.filter(asset => matchesAssetBrowserFilters(asset, assetFilters));
  const matchingAssetIds = new Set(matchingAssets.map(asset => asset.id));
  const pinnedAssets = assets.filter(asset => pinnedAssetIds.has(asset.id) && !matchingAssetIds.has(asset.id));
  const hasActiveAssetFilters = assetFilters.query !== ''
    || assetFilters.kind !== 'all'
    || assetFilters.origin !== 'all';
  const quadArtifact = quadTopologyArtifact(selectedAsset);
  const quadriflowBackend = runtime?.quadriflow_backend;
  const [checkingRetopology, setCheckingRetopology] = useState(false);
  const [retopologyCheck, setRetopologyCheck] = useState<{ assetId: string; message: string } | null>(null);
  const [installingBackend, setInstallingBackend] = useState(false);
  const isRunning = isActiveJob(job);
  const backendStatus = (id: string) => runtime?.model_backends.find((backend) => backend.id === id)?.status.replaceAll('_', ' ') ?? 'Connect engine';
  const mlxBackend = runtime?.model_backends.find((backend) => backend.id === 'pixal3d-mlx');
  const cudaBackend = runtime?.model_backends.find((backend) => backend.id === 'pixal3d-cuda');
  const automaticBackend = runtime?.platform === 'linux' ? cudaBackend : mlxBackend;
  const selectedGenerationBackend = generationBackend === 'auto'
    ? automaticBackend
    : runtime?.model_backends.find((backend) => backend.id === generationBackend);

  async function requestAssetPage(
    client: MeshHarborApi,
    filters: AssetBrowserFilters,
    append: boolean,
  ): Promise<string | null> {
    if (connection !== 'connected') return null;
    const cursor = append ? assetNextCursor : null;
    if (append && !cursor) return null;

    const requestId = assetPageRequestGuard.current.begin();
    assetFiltersRef.current = filters;
    setAssetFilters(filters);
    setAssetPageLoading(true);
    setError(null);
    if (!append) setAssetNextCursor(null);
    try {
      const page = await client.assetPage(ASSET_PAGE_SIZE, cursor, toAssetPageFilters(filters));
      if (!assetPageRequestGuard.current.isCurrent(requestId)) return null;
      setAssets((current) => {
        const selectedPair = comparisonPairRef.current
          ?? assetComparisonPairForSelection(current, selectedIdRef.current, null);
        const preservedIds = [
          selectedPair?.inputAssetId,
          selectedPair?.resultAssetId,
          selectedIdRef.current,
        ];
        const pageIds = new Set(page.items.map((asset) => asset.id));
        const retained = current.filter((asset) =>
          !pageIds.has(asset.id) && (append || preservedIds.includes(asset.id)));
        return [...page.items, ...retained];
      });
      setAssetNextCursor(page.next_cursor);
      return null;
    } catch (cause) {
      if (!assetPageRequestGuard.current.isCurrent(requestId)) return null;
      const message = cause instanceof Error
        ? 'Could not load the asset library. ' + cause.message
        : 'Could not load the asset library.';
      setError(message);
      return message;
    } finally {
      if (assetPageRequestGuard.current.isCurrent(requestId)) setAssetPageLoading(false);
    }
  }

  async function refreshAssets(client: MeshHarborApi) {
    await requestAssetPage(client, assetFilters, false);
  }

  function applyAssetBrowserFilters(filters: AssetBrowserFilters) {
    const normalized = { ...filters, query: clampAssetSearchQuery(filters.query) };
    assetBrowserInteractionRevision.current += 1;
    assetSearchInputRef.current = normalized.query;
    setAssetSearchInput(normalized.query);
    void requestAssetPage(api, normalized, false);
  }

  function invalidateHistoryRequests() {
    historySelectionGuard.current.invalidate();
    recentJobsRequestGuard.current.invalidate();
    setComparisonLoadingSide(null);
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
    setComparisonLoadingSide(null);
    setReportJob(null);
    setError(null);
    setNotice(null);
    try {
      const response = await api.job(jobId);
      if (!historySelectionGuard.current.isCurrent(requestId)) return;
      if (shouldDeferTrackedJobCompletion(jobRef.current, response)) {
        recentJobsRequestGuard.current.invalidate();
        setRecentJobs((current) => [response, ...current.filter((record) => record.id !== response.id)]
          .sort((left, right) => Number(right.updated_at) - Number(left.updated_at)
            || right.id.localeCompare(left.id))
          .slice(0, 20));
        setNotice('The live status monitor will apply this result and refresh runtime readiness. No status tracking was stopped.');
        return;
      }
      const latest = preferTrackedTerminalJob(jobRef.current, response);
      if (!canInspectJobHistoryRecord(jobRef.current, latest)) {
        setNotice('Wait for the active job to finish before opening a different saved result. Its progress and cancellation controls will remain attached.');
        return;
      }

      const active = isActiveJob(latest);
      let inputAsset: Asset | null = null;
      if (latest.input_asset_id
        && (active || !latest.result?.asset_id)
        && !assets.some((asset) => asset.id === latest.input_asset_id)) {
        inputAsset = await api.derivedAsset(latest.input_asset_id);
        if (!historySelectionGuard.current.isCurrent(requestId)) return;
        if (inputAsset.id !== latest.input_asset_id) {
          throw new Error('The saved job input did not match the requested asset.');
        }
      }
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
        setComparisonPair(null);
        if (inputAsset) {
          const loadedInputAsset = inputAsset;
          setAssets((current) => [loadedInputAsset, ...current.filter((item) => item.id !== loadedInputAsset.id)]);
        }
        if (latest.input_asset_id) {
          selectAssetId(latest.input_asset_id);
        }
        setNotice(jobRef.current?.id === latest.id
          ? `The live monitor remains attached to the current ${latest.operation} job; this check did not replace its tracking.`
          : `Last saved state: ${latest.state} (${latest.progress}%). This read-only check did not resume work. Refresh to check again. A saved active state may be stale after an unexpected engine exit; stop every REST/MCP process using this data directory and run meshharbor jobs recover-interrupted --confirm-no-running-meshharbor before retrying.`);
        return;
      }

      setJob(latest);
      setReportJob(latest.state === 'succeeded' ? latest : null);
      setComparisonPair(null);
      if (inputAsset) {
        const loadedInputAsset = inputAsset;
        setAssets((current) => [loadedInputAsset, ...current.filter((item) => item.id !== loadedInputAsset.id)]);
      }
      if (latest.input_asset_id) {
        selectAssetId(latest.input_asset_id);
      }
      if (derived) {
        const resultAsset = derived;
        const pair = assetComparisonPairFromJob(latest, resultAsset);
        setComparisonPair(pair);
        setAssets((current) => [resultAsset, ...current.filter((item) => item.id !== resultAsset.id)]);
        selectAssetId(resultAsset.id);
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
    historySelectionGuard.current.invalidate();
    assetPageRequestGuard.current.invalidate();
    setAssetPageLoading(true);
    setComparisonLoadingSide(null);
    setComparisonPair(null);
    setError(null);
    setConnectionError(null);
    setConnection('connecting');
    try {
      const health = await api.health();
      if (health.service !== 'meshharbor' || health.status !== 'ok') throw new Error('This does not look like a MeshHarbor server.');
      const [runtimeInfo, operationList, assetPage, jobList] = await Promise.all([
        api.runtime(), api.operations(), api.assetPage(ASSET_PAGE_SIZE, null, toAssetPageFilters(assetFiltersRef.current)), api.jobs(20),
      ]);
      setRuntime(runtimeInfo);
      setOperations(operationList);
      setAssets((current) => {
        const selectedPair = comparisonPairRef.current
          ?? assetComparisonPairForSelection(current, selectedIdRef.current, null);
        const pageIds = new Set(assetPage.items.map((asset) => asset.id));
        const preservedIds = new Set([
          selectedPair?.inputAssetId,
          selectedPair?.resultAssetId,
          selectedIdRef.current,
        ]);
        return [
          ...assetPage.items,
          ...current.filter((asset) => preservedIds.has(asset.id) && !pageIds.has(asset.id)),
        ];
      });
      setAssetNextCursor(assetPage.next_cursor);
      setRecentJobs(jobList);
      setSelectedId((current) => {
        const next = current && (assetPage.items.some((item) => item.id === current)
          || current === selectedIdRef.current)
          ? current
          : assetPage.items.find((item) => item.kind === 'mesh')?.id ?? null;
        selectedIdRef.current = next;
        return next;
      });
      setAssetPageLoading(false);
      setConnection('connected');
      setShowConnect(false);
    } catch (cause) {
      setAssetPageLoading(false);
      setConnection('disconnected');
      const message = cause instanceof Error ? cause.message : 'Could not connect to MeshHarbor.';
      setConnectionError(message);
    }
  }, [api]);

  useEffect(() => { void connect(); }, [connect]);

  useEffect(() => {
    selectedIdRef.current = selectedId;
  }, [selectedId]);

  useEffect(() => {
    assetFiltersRef.current = assetFilters;
  }, [assetFilters]);

  useEffect(() => {
    assetSearchInputRef.current = assetSearchInput;
  }, [assetSearchInput]);

  useEffect(() => {
    comparisonPairRef.current = comparisonPair;
  }, [comparisonPair]);

  function selectAssetId(assetId: string | null) {
    assetSelectionInteractionRevision.current += 1;
    selectedIdRef.current = assetId;
    setSelectedId(assetId);
  }

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
            setReportJob(null);
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
          let derived: Asset | null = null;
          if (latest.result?.asset_id) {
            derived = await api.derivedAsset(latest.result.asset_id);
          }
          if (stopped) return;
          setJob(latest);
          setReportJob(latest);
          if (derived) {
            const completedAsset = derived;
            const pair = assetComparisonPairFromJob(latest, completedAsset);
            setComparisonPair(pair);
            setAssets((current) => [completedAsset, ...current.filter((item) => item.id !== completedAsset.id)]);
            if (shouldAutoSelectComparisonResult(pair, selectedIdRef.current)) {
              selectAssetId(completedAsset.id);
              const completedInspector: Inspector = latest.operation === 'mesh.simplify' ? 'optimize' : latest.operation === 'image.generate' ? 'generation' : 'tools';
              const toolPane = document.getElementById('workbench-inspector');
              if (toolPane?.dataset.tool === completedInspector &&
                (document.activeElement === document.body || toolPane.contains(document.activeElement))) {
                if (inspectorTrigger.current?.isConnected) inspectorTrigger.current.focus();
              }
              setInspector(current => current === completedInspector ? null : current);
            }
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

  const previewAsset = selectedAsset;
  // Keep the viewing frame while the active job temporarily hides comparison controls.
  const previewPair = assetComparisonPairForSelection(assets, selectedId, comparisonPair);
  const comparisonSourceId = selectedAsset?.kind === 'mesh'
    ? previewPair?.inputAssetId ?? null
    : null;
  const comparisonSourceInLibrary = comparisonSourceId
    ? assets.find((asset) => asset.id === comparisonSourceId) ?? null
    : null;
  useEffect(() => {
    if (!comparisonSourceId || comparisonSourceInLibrary || connection !== 'connected') return;
    let current = true;
    void api.derivedAsset(comparisonSourceId).then((source) => {
      if (current && source.id === comparisonSourceId) setResolvedPreviewSource(source);
    }).catch(() => {
      // The preview remains usable without shared framing. Selecting Original will surface a load error if necessary.
    });
    return () => { current = false; };
  }, [api, comparisonSourceId, comparisonSourceInLibrary, connection]);
  const previewSource = comparisonSourceInLibrary
    ?? (resolvedPreviewSource?.id === comparisonSourceId ? resolvedPreviewSource : null);
  const comparisonResultId = previewPair?.resultAssetId;
  const previewComparison = previewSource?.kind === 'mesh' && comparisonResultId
    && (selectedId === previewSource.id || selectedId === comparisonResultId)
    && assets.some((asset) => asset.id === comparisonResultId
      && asset.kind === 'mesh' && asset.source_asset_id === previewSource.id)
    ? {
      groupId: `${previewSource.id}:${comparisonResultId}`,
      sourceAsset: previewSource,
      resultAssetId: comparisonResultId,
      side: selectedId === previewSource.id ? 'source' as const : 'result' as const,
    }
    : null;

  const loadPreview = useCallback((asset: Asset) => {
    if (connection !== 'connected') return Promise.reject(new Error('The local MeshHarbor engine is offline.'));
    return api.artifact(asset);
  }, [api, connection]);
  const reportPreviewError = useCallback((message: string) => setError(`Preview: ${message}`), []);

  function selectAsset(asset: Asset) {
    if (asset.id !== selectedId) setInspector(asset.kind === 'image' && !inspectorIsSheet ? 'generation' : null);
    invalidateHistoryRequests();
    const pair = assetComparisonPairForSelection(assets, asset.id, comparisonPair);
    setComparisonPair(pair);
    selectAssetId(asset.id);
    setReportJob(null);
  }

  async function selectComparisonSide(side: AssetComparisonSide) {
    const pair = selectedComparisonPair;
    if (!pair || connection !== 'connected' || busy || comparisonLoadingSide) return;
    const requestId = historySelectionGuard.current.begin();
    setComparisonPair(pair);
    setComparisonLoadingSide(side);
    setError(null);
    try {
      const asset = await resolveComparisonAsset(pair, side, assets, (assetId) => api.derivedAsset(assetId));
      if (!historySelectionGuard.current.isCurrent(requestId)) return;
      setAssets((current) => [asset, ...current.filter((item) => item.id !== asset.id)]);
      selectAssetId(asset.id);
    } catch (cause) {
      if (!historySelectionGuard.current.isCurrent(requestId)) return;
      const label = side === 'source' ? 'original' : 'result';
      setError(cause instanceof Error ? `Could not load the comparison ${label}. ${cause.message}` : `Could not load the comparison ${label}.`);
    } finally {
      if (historySelectionGuard.current.isCurrent(requestId)) setComparisonLoadingSide(null);
    }
  }

  async function importFiles(files: FileList | File[]) {
    if (files.length === 0) return;
    if (importBatchInFlight.current) {
      setNotice('An import batch is already running. These additional files were not queued.');
      return;
    }
    if (busy) {
      setError('Wait for the current Studio operation to finish before importing files.');
      return;
    }
    if (connection !== 'connected') {
      setShowConnect(true);
      setConnectionError('The engine must be running before Studio can import files.');
      return;
    }

    importBatchInFlight.current = true;
    const started = {
      selectedId: selectedIdRef.current,
      filters: { ...assetFiltersRef.current },
      searchInput: assetSearchInputRef.current,
      browserInteractionRevision: assetBrowserInteractionRevision.current,
      selectionInteractionRevision: assetSelectionInteractionRevision.current,
    };
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const result = await importAssetBatch(files, async (file, kind) =>
        kind === 'image' ? api.importImage(file) : api.importGlb(file),
      (progress) => {
        setNotice(`Importing ${progress.current} of ${progress.total}: ${progress.fileName}`);
      });
      let refreshError: string | null = null;
      let filtersToRefresh = assetFiltersRef.current;
      if (result.assets.length > 0) {
        setAssets((current) => {
          const importedIds = new Set(result.assets.map((asset) => asset.id));
          return [...result.assets, ...current.filter((asset) => !importedIds.has(asset.id))];
        });

        const current = {
          selectedId: selectedIdRef.current,
          filters: { ...assetFiltersRef.current },
          searchInput: assetSearchInputRef.current,
          browserInteractionRevision: assetBrowserInteractionRevision.current,
          selectionInteractionRevision: assetSelectionInteractionRevision.current,
        };
        const plan = planAssetImportUiCompletion(started, current);
        if (plan.selectFirstImportedAsset) {
          invalidateHistoryRequests();
          comparisonPairRef.current = null;
          setComparisonPair(null);
          setInspector(result.assets[0].kind === 'image' && !inspectorIsSheet ? 'generation' : null);
          selectAssetId(result.assets[0].id);
          setReportJob(null);
        }

        if (plan.resetFilters) {
          filtersToRefresh = EMPTY_ASSET_BROWSER_FILTERS;
          assetSearchInputRef.current = '';
          setAssetSearchInput('');
        }
      }
      if (result.attemptedCount > 0) {
        // Reconcile ambiguous failures where the server may have stored an asset
        // before the response was lost, without ever replaying the upload.
        refreshError = await requestAssetPage(api, filtersToRefresh, false);
      }

      const consideredCount = Math.min(files.length, MAX_ASSET_IMPORT_FILES);
      const summaryParts = result.assets.length > 0 ? [result.assets.length === consideredCount
        ? `Imported ${result.assets.length} file${result.assets.length === 1 ? '' : 's'}.`
        : `Imported ${result.assets.length} of ${consideredCount} files.`] : [];
      const shownFailures = result.failures.slice(0, 3);
      if (shownFailures.length > 0) {
        const failureText = shownFailures.map((failure) => {
          const detail = failure.message.length > 180 ? `${failure.message.slice(0, 180)}…` : failure.message;
          return `${failure.fileName}: ${detail}`;
        }).join(' ');
        const moreFailures = result.failures.length - shownFailures.length;
        summaryParts.push(result.failures.length === 1 ? failureText : `Could not import ${result.failures.length} files: ${failureText}${moreFailures > 0 ? ` And ${moreFailures} more.` : ''}`);
      }
      if (result.skippedCount > 0) {
        summaryParts.push(`${result.skippedCount} additional file${result.skippedCount === 1 ? ' was' : 's were'} not processed; the per-batch limit is ${MAX_ASSET_IMPORT_FILES}.`);
      }
      if (refreshError) summaryParts.push(refreshError);
      const summary = summaryParts.join(' ');
      if (result.failures.length > 0 || result.skippedCount > 0 || refreshError) {
        setError(summary);
      } else {
        // The active file already confirms a single successful import.
        setNotice(result.assets.length > 1 ? summary : null);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not import these files.');
    } finally {
      importBatchInFlight.current = false;
      setBusy(false);
    }
  }

  async function simplify() {
    if (!selectedAsset) return;
    invalidateHistoryRequests();
    setError(null);
    setNotice(null);
    setBusy(true);
    setReportJob(null);
    setComparisonPair(null);
    try {
      const nextJob = await api.createSimplifyJob(selectedAsset.id, Math.max(1, Math.floor(targetTriangles)), maxError);
      setJob(nextJob);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not start simplification.');
    } finally {
      setBusy(false);
    }
  }

  async function checkRetopologyInput() {
    if (!selectedAsset || selectedAsset.kind !== 'mesh' || checkingRetopology) return;
    const assetId = selectedAsset.id;
    setCheckingRetopology(true);
    setRetopologyCheck(null);
    try {
      const report = await api.preflightRetopology(assetId);
      const input = report.appearance_projection_required ? 'Textured input' : 'Geometry-only input';
      const seams = report.coincident_boundary_vertices_joined ? ' Exact seams can be joined for the solver without changing the source.' : '';
      setRetopologyCheck({ assetId, message: `${input} passed the input checks (${report.input_triangles.toLocaleString()} triangles).${seams} The solver was not run.` });
    } catch (cause) {
      setRetopologyCheck({ assetId, message: cause instanceof Error ? cause.message : 'Could not check the retopology input.' });
    } finally {
      setCheckingRetopology(false);
    }
  }

  async function runMeshOperation(operation: StudioMeshOperationId) {
    if (!selectedAsset || selectedAsset.kind !== 'mesh') return;
    invalidateHistoryRequests();
    setError(null);
    setNotice(null);
    setBusy(true);
    setReportJob(null);
    setComparisonPair(null);
    try {
      const parameters: Record<string, number> = operation === 'render.optimize'
        ? { overdraw_threshold: 1.05 }
        : {};
      const nextJob = await api.createMeshOperationJob(selectedAsset.id, operation, parameters);
      setJob(nextJob);
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
    setReportJob(null);
    setComparisonPair(null);
    try {
      const nextJob = await api.createImageGenerationJob(selectedAsset.id, generationBackend, textureSize, Math.max(4, Math.floor(targetFaces)));
      setJob(nextJob);
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
  const isComparingResult = selectedComparisonPair
    ? selectedComparisonPair.resultAssetId === selectedId
    : Boolean(selectedAsset?.source_asset_id);
  const activeView: AssetComparisonSide = isComparingResult ? 'result' : 'source';
  const openInspector = inspectorForAsset(inspector, selectedAsset?.kind);
  const comparisonResult = previewComparison
    ? assets.find(asset => asset.id === previewComparison.resultAssetId)
    : null;
  const beforeTriangles = previewComparison?.sourceAsset.metadata.triangles;
  const afterTriangles = comparisonResult?.metadata.triangles;
  const comparisonSizes = previewComparison && comparisonResult
    ? formatVersionSizes(previewComparison.sourceAsset.byte_size, comparisonResult.byte_size)
    : null;
  const showVersionStats = previewComparison && comparisonResult
    && (typeof beforeTriangles === 'number' || comparisonSizes);
  function toggleInspector(next: Inspector, trigger: HTMLButtonElement) {
    inspectorTrigger.current = trigger;
    setInspector(current => current === next ? null : next);
  }
  const inspectorTitle = openInspector === 'generation' ? 'Generate model' : openInspector === 'optimize' ? 'Optimize mesh' : 'Mesh tools';
  const showInspectorSheet = inspectorIsSheet && !!openInspector;
  const exportIsPrimary = shouldHighlightExport(activeView, openInspector, isRunning, connection === 'connected');
  function restoreInspectorFocus() {
    const trigger = inspectorTrigger.current;
    if (trigger?.isConnected && trigger.getClientRects().length) trigger.focus();
    else workbenchNode?.querySelector<HTMLButtonElement>('.workbench-view-actions button')?.focus();
  }
  function closeInspector() {
    setInspector(null);
    restoreInspectorFocus();
  }
  const jobFeedback = isRunning ? (
    <div className="workspace-job">
      <div className="workspace-job-copy" role="status">
        <span className="spinner small" aria-hidden="true" />
        <strong>{job.state === 'queued' ? 'Queued' : job.state === 'validating' ? 'Validating' : job.state === 'cancelling' ? 'Cancelling' : 'Working'}</strong>
        <span>{studioOperationLabel(job.operation)} · {job.progress}%</span>
      </div>
      <Progress value={job.progress} aria-label="Job progress" />
      {isQueuedJobCancellable(job) && <Button variant="outline" size="sm" className="cancel-job" onClick={() => void cancelJob()}>Cancel queued job</Button>}
    </div>
  ) : null;
  const feedback = !error && reportCard && exportIsPrimary && notice ? (
    <span className="sr-only" role="status">{notice}</span>
  ) : (error || notice) ? (
    <Alert className={`toast ${error ? 'toast-error' : 'toast-success'}`} variant={error ? 'destructive' : 'default'} role={error ? 'alert' : 'status'}>
      {error && <CircleAlert aria-hidden="true" />}
      <AlertDescription>{error ?? notice}</AlertDescription>
      <Button variant="ghost" size="icon-sm" onClick={() => { setError(null); closeNotice(); }} aria-label="Dismiss"><X /></Button>
    </Alert>
  ) : null;

  function renderAssetRow(asset: Asset) {
    return (
      <Button key={asset.id} variant="ghost" aria-pressed={asset.id === selectedId}
        className={`asset-row ${asset.id === selectedId ? 'selected' : ''}`}
        onClick={() => { selectAsset(asset); if (sidebar.narrow) sidebar.setPanel(null); }}>
        <span className="asset-row-icon">{asset.kind === 'mesh' ? <Box aria-hidden="true" /> : <FileImage aria-hidden="true" />}</span>
        <span className="asset-row-name">
          <strong title={asset.name}>{asset.name}</strong>
          <small>{asset.source_asset_id ? 'Result' : 'Original'} · {formatSize(asset.byte_size)}</small>
        </span>
        {asset.kind === 'mesh' && <span className="asset-row-tris">{Number(asset.metadata.triangles ?? 0).toLocaleString()} <small>tris</small></span>}
      </Button>
    );
  }

  function inspectorBody() {
    return <>
      {openInspector === 'optimize' && <div className="operation-card">
        <p className="operation-description">Reduce polygon count while keeping the original file intact and preserving material references.</p>
        <div data-slot="field" className="field-label-row"><Label htmlFor="triangle-target">Triangle target</Label><span className="field-hint">Goal, not a guarantee</span></div>
        <div className="number-input-wrap"><Input id="triangle-target" type="number" min="1" max="10000000" step="1" value={targetTriangles} onChange={(event) => setTargetTriangles(Number(event.target.value))} disabled={!selectedAsset || isRunning || busy} /><span>tris</span></div>
        <div className="range-row"><span>Smaller</span><Slider label="Triangle target" min={1} max={Math.max(4, triangleCount || 20000)} step={1} value={[Math.max(1, Math.min(targetTriangles, Math.max(4, triangleCount || 20000)))]} onValueChange={([value]) => setTargetTriangles(value)} disabled={!selectedAsset || isRunning || busy} /><span>More detail</span></div>
        <details data-slot="collapsible" className="settings-disclosure"><summary>Shape protection</summary>
        <div className="field-label-row tolerance-label"><span>Error tolerance</span><span className="mono-value">{maxError.toFixed(3)}</span></div>
        <Slider id="error-budget" label="Maximum geometric error" min={0.001} max={0.08} step={0.001} value={[maxError]} onValueChange={([value]) => setMaxError(value)} disabled={!selectedAsset || isRunning || busy} />
        <div className="control-footnote">A lower tolerance may stop above your target to protect shape.</div>
        </details>
        <div className="preserve-note"><span className="preserve-check">✓</span><span>Keeps the original file</span><span className="preserve-check">✓</span><span>Keeps UV and material references</span></div>
        {!(isRunning && job.operation === 'mesh.simplify') && <Button className="simplify-button" disabled={!canSimplify || busy || connection !== 'connected' || isRunning} onClick={() => void simplify()}>{busy ? <><span className="spinner small" /> Working…</> : <><SlidersHorizontal aria-hidden="true" /> Optimize mesh <span className="button-arrow">→</span></>}</Button>}
      </div>}

      {openInspector === 'generation' && <section className="operation-card generation-card" id="image-generation">
          {!canGenerate && <p className="generation-unavailable">{selectedGenerationBackend?.installable
            ? 'Set up a local runtime to generate this model. You can still work with GLBs.'
            : 'Image-to-3D is unavailable on this computer. You can still work with GLBs.'}</p>}
          <details data-slot="collapsible" className="settings-disclosure"><summary>Model settings</summary>
          <div data-slot="field" className="field-label-row"><Label htmlFor="generation-backend">Backend</Label><span className="field-hint">Auto selects this host’s backend</span></div>
          <NativeSelect id="generation-backend" value={generationBackend} onChange={(event) => setGenerationBackend(event.target.value)} disabled={!selectedAsset || selectedAsset.kind !== 'image' || busy || isRunning}>
            <option value="auto">Auto ({automaticBackend?.id === 'pixal3d-cuda' ? 'CUDA' : 'MLX'})</option>
            <option value="pixal3d-mlx">Pixal3D MLX (Apple Silicon)</option>
            <option value="pixal3d-cuda">Pixal3D CUDA (Linux/NVIDIA)</option>
          </NativeSelect>
          <div data-slot="field" className="field-label-row"><Label htmlFor="texture-size">Texture size</Label><span className="field-hint">Higher uses more memory</span></div>
          <NativeSelect id="texture-size" value={textureSize} onChange={(event) => setTextureSize(Number(event.target.value))} disabled={!selectedAsset || selectedAsset.kind !== 'image' || busy || isRunning}>
            <option value={512}>512</option><option value={1024}>1024</option><option value={2048}>2048</option>
          </NativeSelect>
          <div className="field-label-row generation-face-label"><Label htmlFor="target-faces">Soft face target</Label><span className="field-hint">Not a hard cap</span></div>
          <div className="number-input-wrap"><Input id="target-faces" type="number" min="4" max="1000000" step="1000" value={targetFaces} onChange={(event) => setTargetFaces(Number(event.target.value))} disabled={!selectedAsset || selectedAsset.kind !== 'image' || busy || isRunning} /><span>faces</span></div>
          </details>
          {canGenerate && !(isRunning && job.operation === 'image.generate') && <Button className="simplify-button" disabled={busy || connection !== 'connected' || isRunning} onClick={() => void generate()}>{busy ? <><span className="spinner small" /> Working…</> : <>Generate 3D <span className="button-arrow">→</span></>}</Button>}
          <details data-slot="collapsible" className="settings-disclosure runtime-disclosure" open={!canGenerate || undefined}>
            <summary>{canGenerate ? 'Local runtime' : 'Runtime setup and compatibility'}</summary>
          <div className="runtime-setup-panel">
            {(selectedGenerationBackend?.id === 'pixal3d-mlx' || (isRunning && (job.operation === 'runtime.pixal3d.setup' || job.operation === 'runtime.pixal3d.install_models' || job.operation === 'runtime.pixal3d.convert_naf'))) && <>
            <div className="runtime-panel-heading">Pixal3D MLX setup <span>{mlxBackend?.status.replaceAll('_', ' ') ?? 'engine disconnected'}</span></div>

            {!isRunning && mlxBackend?.installable && !mlxBackend.environment_ready && <><p>MeshHarbor will create a private Python 3.12 environment and install the pinned backend dependencies. Install <a href="https://docs.astral.sh/uv/getting-started/installation/" target="_blank" rel="noreferrer">uv</a> first if it is not already on PATH.</p><Button variant="secondary" size="sm" className="runtime-action" disabled={busy || isRunning || connection !== 'connected'} onClick={() => void setupPixal3dRuntime()}>{busy ? 'Starting…' : 'Set up Python environment'} <span className="button-arrow">→</span></Button></>}
            {!isRunning && mlxBackend?.environment_ready && !mlxBackend.models_downloaded && <><p>Sign in in a separate local terminal with <code>meshharbor runtime hf-login</code>. The token is entered through Hugging Face's masked prompt and stored only in MeshHarbor's isolated cache.</p><p className="runtime-terms">MeshHarbor’s project license has not been selected in this repository, and model-checkpoint terms remain separate. Review the DINOv3 and NAF checkpoint terms and obtain gated access before continuing. This download uses about 24 GB of disk space, then installs an isolated CPU-only NAF conversion environment.</p><Label className="runtime-consent"><Checkbox checked={modelDownloadAcknowledged} onCheckedChange={value => setModelDownloadAcknowledged(value === true)} disabled={busy || isRunning || connection !== 'connected'} /><span>I have reviewed the model terms, have access to gated DINOv3, and am ready to download the files to this computer.</span></Label><Button variant="secondary" size="sm" className="runtime-action" disabled={!mlxBackend.downloads_enabled || !modelDownloadAcknowledged || busy || isRunning || connection !== 'connected'} onClick={() => void installPixal3dModels()}>{busy ? 'Starting…' : 'Download model files'} <span className="button-arrow">↓</span></Button></>}
            {!isRunning && mlxBackend?.models_downloaded && mlxBackend.naf_conversion_required && <div className="runtime-conversion"><strong>One setup step remains</strong><span>The verified NAF checkpoint needs one-time conversion. MeshHarbor installs the pinned CPU-only converter into a separate environment.</span><Button variant="secondary" size="sm" className="runtime-action" disabled={!mlxBackend.conversion_enabled || busy || isRunning || connection !== 'connected'} onClick={() => void convertPixal3dNaf()}>{busy ? 'Starting…' : 'Finish NAF conversion'} <span className="button-arrow">→</span></Button></div>}
            {!isRunning && mlxBackend?.installable === false && <p>Pixal3D MLX setup is supported on Apple Silicon macOS only. CUDA setup is a separate Linux/NVIDIA runtime.</p>}
            </>}
            {(selectedGenerationBackend?.id === 'pixal3d-cuda' || (isRunning && (job.operation === 'runtime.pixal3d.cuda_setup' || job.operation === 'runtime.pixal3d.cuda_install_models'))) && <>
            <div className="runtime-panel-heading">Pixal3D CUDA setup <span>{cudaBackend?.status.replaceAll('_', ' ') ?? 'engine disconnected'}</span></div>

            {!isRunning && cudaBackend?.installable && !cudaBackend.environment_ready && <><p>On Linux x86_64 with an NVIDIA GPU, MeshHarbor will create a private Python 3.12 environment, resolve upstream dependency ranges into an app-private lock, install the CUDA 12.4 profile, and run a CUDA/SDPA preflight. This does not download model weights.</p><Button variant="secondary" size="sm" className="runtime-action" disabled={busy || isRunning || connection !== 'connected'} onClick={() => void setupPixal3dCudaRuntime()}>{busy ? 'Starting…' : 'Set up CUDA environment'} <span className="button-arrow">→</span></Button></>}
            {!isRunning && cudaBackend?.environment_ready && !cudaBackend.models_downloaded && <><p>Sign in in a separate local terminal with <code>meshharbor runtime cuda-hf-login</code>. Hugging Face's prompt stores credentials only in the isolated CUDA cache.</p><p className="runtime-terms">MeshHarbor’s project license has not been selected in this repository; model/checkpoint terms are separate. Review DINOv3, Pixal3D, and NAF terms and obtain gated DINOv3 access first. The download is about 25–30 GB and needs roughly 40 GiB of free disk space.</p><Label className="runtime-consent"><Checkbox checked={cudaModelDownloadAcknowledged} onCheckedChange={value => setCudaModelDownloadAcknowledged(value === true)} disabled={busy || isRunning || connection !== 'connected'} /><span>I reviewed the model terms, have gated DINOv3 access, and approve downloading the CUDA model files to this computer.</span></Label><Button variant="secondary" size="sm" className="runtime-action" disabled={!cudaBackend.downloads_enabled || !cudaModelDownloadAcknowledged || busy || isRunning || connection !== 'connected'} onClick={() => void installPixal3dCudaModels()}>{busy ? 'Starting…' : 'Download CUDA model files'} <span className="button-arrow">↓</span></Button></>}
            {!isRunning && cudaBackend?.installable === false && <p>Pixal3D CUDA setup is Linux x86_64/NVIDIA only. Use the Apple MLX backend on supported Macs; the CUDA backend is not available on this host.</p>}
            {cudaBackend?.license_notice && <p className="runtime-terms">{cudaBackend.license_notice}</p>}
            </>}
            {!selectedGenerationBackend && <p>No compatible image-to-3D backend was detected. You can still work with existing GLBs.</p>}
            {isRunning && job.operation.startsWith('runtime.') && <small className="runtime-serial-note">Runtime installation and generation share one local worker.</small>}
          </div>
          </details>
        </section>}

      {openInspector === 'tools' && <>
      <div className="operation-card mesh-tools-card" id="mesh-tools">
        <p className="operation-description">Mesh processing creates a new asset. Input checks are read-only. Your source file is never overwritten.</p>
        <div className="mesh-tool-list">
          {meshOperationOptions.map((tool) => {
            const available = operations.find((operation) => operation.id === tool.id)?.implemented === true;
            const processing = isRunning && job.operation === tool.id;
            return (
              <details className="mesh-tool-item settings-disclosure" name="mesh-tool" key={tool.id}>
                <summary>{tool.title}</summary>
                <p>{tool.description}</p>
                {tool.id === 'mesh.retopology' && (
                  <>
                    <Button
                      variant="secondary" size="sm" className="mesh-tool-button"
                      disabled={!selectedAsset || selectedAsset.kind !== 'mesh' || busy || checkingRetopology || connection !== 'connected' || isRunning}
                      onClick={() => void checkRetopologyInput()}
                    >
                      {checkingRetopology ? 'Checking input…' : 'Check retopology input'}
                    </Button>
                    {retopologyCheck && retopologyCheck.assetId === selectedAsset?.id && (
                      <p role="status">{retopologyCheck.message}</p>
                    )}
                  </>
                )}
                {!processing && (
                  <Button
                    className="mesh-tool-button"
                    disabled={!selectedAsset || selectedAsset.kind !== 'mesh' || !available || busy || checkingRetopology || connection !== 'connected' || isRunning}
                    onClick={() => void runMeshOperation(tool.id)}
                  >
                    Run {tool.title}<span className="button-arrow">→</span>
                  </Button>
                )}
              </details>
            );
          })}
        </div>
      </div>

      <details className="capability-card settings-disclosure"><summary>Retopology availability</summary><p>General high-to-low texture baking remains unavailable. Experimental QuadriFlow retopology is unavailable until a signature-verified, manifest-matched managed sidecar is supported; adjacent binaries are intentionally ignored. Editable quads are preserved in a separate geometry-only OBJ, while supported base-color/metallic-roughness maps are projected only into the triangulated GLB preview. Mesh tools create derived assets and keep originals unchanged. The repository’s project license has not been selected; upstream model terms remain separate.</p><div className="capability-foot"><span>QuadriFlow backend</span><span className="capability-status">{quadriflowBackend?.status.replaceAll('_', ' ') ?? 'Connect engine'}</span></div>{quadriflowBackend && <><p>{quadriflowBackend.message}</p><p>{quadriflowBackend.next_step}</p></>}<Button onClick={() => void installQuadriflowBackend()} disabled={!quadriflowBackend?.install_available || installingBackend}>{installingBackend ? 'Installing…' : quadriflowBackend?.install_available ? 'Install QuadriFlow' : 'Install unavailable'}</Button><div className="capability-foot"><span>CUDA backend</span><span className="capability-status">{backendStatus('pixal3d-cuda')}</span></div><div className="capability-foot"><span>Apple MLX backend</span><span className="capability-status">{backendStatus('pixal3d-mlx')}</span></div></details>
      </>}
    </>;
  }

  return (
    <Dialog open={showConnect} onOpenChange={setShowConnect}>
      <div className="app-shell studio-app mh-shadcn-theme" onDragOver={(event) => {
      if (!event.dataTransfer.types.includes('Files')) return;
      event.preventDefault();
      setDragging(true);
    }} onDragLeave={(event) => {
      if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setDragging(false);
    }} onDrop={(event) => {
      if (!event.dataTransfer.types.includes('Files')) return;
      event.preventDefault();
      setDragging(false);
      void importFiles(event.dataTransfer.files);
    }}>
      <header className="topbar studio-topbar">
        <div className="brand-lockup">
          <div className="brand-mark"><span /><span /><span /></div>
          <span className="brand-name">mesh<span>harbor</span></span>
          <span className="brand-divider" />
          <span className="product-label">Studio</span>
        </div>
          {selectedAsset && <div className="import-actions studio-file-actions" role="group" aria-label="File actions">
            <Button variant="secondary" size="sm" className="import-button" aria-label="New from image" title="New from image" onClick={() => imageInputRef.current?.click()} disabled={busy}><ImagePlus aria-hidden="true" /> <span>New from image</span></Button>
            <Button variant="secondary" size="sm" className="import-button" aria-label="Open GLB" title="Open GLB" onClick={() => fileInputRef.current?.click()} disabled={busy}><Box aria-hidden="true" /> <span>Open GLB</span></Button>
            {quadArtifact && <Button variant="secondary" size="sm" className="import-button" title="Editable quad topology without textures or UVs" disabled={downloading || comparisonLoadingSide !== null || connection !== 'connected'} onClick={() => void downloadResult(quadArtifact)}>Quad OBJ</Button>}
            {selectedAsset.kind === 'mesh' && <Button variant={exportIsPrimary ? 'default' : 'secondary'} size="sm" className="export-glb" data-review-action={exportIsPrimary || undefined} title="Export GLB" aria-label="Export selected GLB" disabled={downloading || comparisonLoadingSide !== null || connection !== 'connected'} onClick={() => void downloadResult()}><Download aria-hidden="true" /> <span>Export GLB</span></Button>}
          </div>}
        <div className="topbar-actions">
          <div className={`engine-status ${connection}`} role="status" title={runtime ? `${runtime.platform} · ${runtime.status.replaceAll('_', ' ')}` : undefined}><i />{connection === 'connected' ? 'Local engine ready' : connection === 'connecting' ? 'Connecting…' : 'Engine offline'}</div>
          <DialogTrigger asChild><Button variant="ghost" size="icon-sm" className="engine-trigger" aria-label="Connect engine" title="Connect local engine"><Plug aria-hidden="true" /></Button></DialogTrigger>
        </div>
      </header>

      <main className="workbench fullscreen-workbench" ref={setWorkbenchNode}>
          <h1 className="sr-only">MeshHarbor Studio</h1>
          <input ref={fileInputRef} className="sr-only" aria-label="Choose GLB files" tabIndex={-1} type="file" multiple accept=".glb,model/gltf-binary" onChange={(event) => { if (event.currentTarget.files) void importFiles(event.currentTarget.files); event.currentTarget.value = ''; }} />
          <input ref={imageInputRef} className="sr-only" aria-label="Choose images" tabIndex={-1} type="file" multiple accept="image/png,image/jpeg,.png,.jpg,.jpeg" onChange={(event) => { if (event.currentTarget.files) void importFiles(event.currentTarget.files); event.currentTarget.value = ''; }} />
          {!showInspectorSheet && !showConnect && <div className="workbench-feedback">
            {feedback}
            {jobFeedback}
          </div>}

          <div className="workbench-grid"
            data-sidebar={sidebar.panel ? 'expanded' : 'collapsed'}
            data-inspector={openInspector && !inspectorIsSheet ? 'open' : 'closed'}>
            <WorkbenchSidebar state={sidebar} portal={workbenchNode}
              assets={
              <div className="library-section" id="asset-library">
                <div className="section-heading">
                  <span>Local library</span>
                  <Button variant="ghost" size="sm" className="text-button" onClick={() => void refreshAssets(api)} disabled={connection !== 'connected'}>Refresh <RotateCw aria-hidden="true" /></Button>
                </div>
                <form className="asset-search" onSubmit={(event) => {
                  event.preventDefault();
                  applyAssetBrowserFilters({ ...assetFilters, query: assetSearchInput.trim() });
                }}>
                  <Label className="asset-search-input">
                    <span>Filename</span>
                    <Input
                      aria-label="Search assets by filename"
                      type="search"
                      maxLength={256}
                      value={assetSearchInput}
                      onChange={(event) => {
                        assetBrowserInteractionRevision.current += 1;
                        assetSearchInputRef.current = event.currentTarget.value;
                        setAssetSearchInput(event.currentTarget.value);
                      }}
                      placeholder="Search assets"
                    />
                  </Label>
                  <Button type="submit" variant="outline" size="sm" className="asset-filter-button" disabled={connection !== 'connected'}>Search</Button>
                </form>
                <div className="asset-filters">
                  <Label>
                    <span>Type</span>
                    <NativeSelect
                      aria-label="Filter assets by type"
                      value={assetFilters.kind}
                      disabled={connection !== 'connected'}
                      onChange={(event) => applyAssetBrowserFilters({
                        ...assetFilters,
                        query: assetSearchInput.trim(),
                        kind: event.currentTarget.value as AssetKindFilter,
                      })}
                    >
                      <option value="all">All types</option>
                      <option value="mesh">3D models</option>
                      <option value="image">Images</option>
                    </NativeSelect>
                  </Label>
                  <Label>
                    <span>Origin</span>
                    <NativeSelect
                      aria-label="Filter assets by origin"
                      value={assetFilters.origin}
                      disabled={connection !== 'connected'}
                      onChange={(event) => applyAssetBrowserFilters({
                        ...assetFilters,
                        query: assetSearchInput.trim(),
                        origin: event.currentTarget.value as AssetOriginFilter,
                      })}
                    >
                      <option value="all">All assets</option>
                      <option value="original">Originals</option>
                      <option value="derived">Generated results</option>
                    </NativeSelect>
                  </Label>
                  {hasActiveAssetFilters && (
                    <Button type="button" variant="ghost" size="sm" className="text-button asset-reset" onClick={() => applyAssetBrowserFilters(EMPTY_ASSET_BROWSER_FILTERS)}>
                      Clear filters
                    </Button>
                  )}
                </div>
                {matchingAssets.length > 0 && <p className="asset-library-note" aria-live="polite">
                  {assetPageLoading ? 'Loading assets…' : `${matchingAssets.length} loaded ${hasActiveAssetFilters ? 'result' : 'asset'}${matchingAssets.length === 1 ? '' : 's'}`}
                </p>}
                {matchingAssets.length === 0
                  ? <div className="library-empty" role="status">
                      {assetPageLoading ? 'Loading assets…' : hasActiveAssetFilters
                        ? 'No matching assets' : 'Imported files and results appear here'}
                    </div>
                  : <div className="asset-list">{matchingAssets.map(renderAssetRow)}</div>}
                {pinnedAssets.length > 0 && <section className="asset-pinned" aria-label="Current selection and comparison">
                  <h3 className="asset-library-note">Current selection</h3>
                  <div className="asset-list">{pinnedAssets.map(renderAssetRow)}</div>
                </section>}
                {assetNextCursor && (
                  <div className="asset-load-more">
                    <Button type="button" variant="outline" size="sm" className="asset-filter-button" onClick={() => void requestAssetPage(api, assetFilters, true)} disabled={assetPageLoading || connection !== 'connected'}>
                      {assetPageLoading ? 'Loading…' : 'Load more assets'}
                    </Button>
                  </div>
                )}
              </div>
              }
              jobs={
              <section className="job-history" aria-label="Recent jobs">
                <div className="section-heading">
                  <div><span>Recent activity</span></div>
                  <Button variant="ghost" size="sm" className="text-button" onClick={() => void refreshRecentJobs()} disabled={connection !== 'connected'}>Refresh <RotateCw aria-hidden="true" /></Button>
                </div>
                {recentJobs.some(savedJob => isActiveJob(savedJob) && savedJob.id !== job?.id) &&
                  <details className="job-history-note"><summary>Recover an interrupted job</summary>
                    <p>Checking a record does not resume it. If the engine exited unexpectedly, stop all REST/MCP processes for this data directory before running <code>meshharbor jobs recover-interrupted --confirm-no-running-meshharbor</code>.</p>
                  </details>}
                {recentJobs.length === 0
                  ? <div className="job-history-empty">No saved jobs yet.</div>
                  : <div className="job-history-list">{recentJobs.slice(0, 8).map((savedJob) => (
                    <div className={`job-history-row ${savedJob.id === job?.id ? 'selected' : ''}`} key={savedJob.id}>
                      <div className="job-history-copy"><strong>{studioOperationLabel(savedJob.operation)}</strong><small>Updated {formatJobTime(savedJob.updated_at)}</small></div>
                      <div className="job-history-status" data-state={savedJob.state}><strong>{savedJob.state === 'succeeded' ? 'Complete' : savedJob.state[0].toUpperCase() + savedJob.state.slice(1)}</strong>{isActiveJob(savedJob) && <span>{savedJob.progress}%</span>}</div>
                      <Button variant="ghost" size="sm" className="job-history-open" onClick={() => { void openRecentJob(savedJob.id); if (sidebar.narrow) sidebar.setPanel(null); }} disabled={connection !== 'connected' || (isRunning && savedJob.id !== job?.id)}>{['queued', 'running', 'validating', 'cancelling'].includes(savedJob.state) ? 'Check' : 'View'}</Button>
                    </div>
                  ))}</div>}
              </section>
              }
            />

            <section className="viewer-column workbench-viewer" aria-label={selectedAsset ? 'Selected asset' : 'Start a model'}>
              {!selectedAsset ? <div className="studio-start">
                <h2>Open a model or image</h2>
                <p>Drop a GLB, PNG or JPEG to start.</p>
                <div className="studio-start-actions">
                  <Button onClick={() => imageInputRef.current?.click()} disabled={busy}><ImagePlus aria-hidden="true" /> Create from image</Button>
                  <Button variant="secondary" size="sm" onClick={() => fileInputRef.current?.click()} disabled={busy}><Box aria-hidden="true" /> <span>Open GLB</span></Button>
                </div>
              </div> : <div className="viewer-card">
                <div className="workbench-commandbar">
                <div className="viewer-toolbar">
                  <div className="viewer-title">{selectedAsset.kind === 'image' ? <FileImage aria-hidden="true" /> : <Box aria-hidden="true" />} <span className="viewer-filename" title={selectedAsset.name}>{selectedAsset.name}</span></div>
                  {selectedComparisonPair && (
                    <ToggleGroup className="view-toggle" type="single" orientation="horizontal"
                      aria-label="Preview version" aria-busy={comparisonLoadingSide !== null}
                      disabled={comparisonLoadingSide !== null || busy || connection !== 'connected'}
                      value={activeView} onValueChange={value => {
                        if ((value === 'source' || value === 'result') && value !== activeView) void selectComparisonSide(value);
                      }}>
                      <ToggleGroupItem className={activeView === 'source' ? 'active' : ''} value="source">Original</ToggleGroupItem>
                      <ToggleGroupItem className={activeView === 'result' ? 'active' : ''} value="result">Result</ToggleGroupItem>
                    </ToggleGroup>
                  )}
                </div>
                <div className="workbench-view-actions" role="group" aria-label="Model tools">
                {selectedAsset.kind === 'image' ? (
                  <Button variant="secondary" size="sm" aria-expanded={openInspector === 'generation'} aria-haspopup={inspectorIsSheet ? 'dialog' : undefined} aria-controls={openInspector === 'generation' ? 'workbench-inspector' : undefined} onClick={event => toggleInspector('generation', event.currentTarget)}><SlidersHorizontal aria-hidden="true" /> Generation settings</Button>
                ) : (
                  <>
                    <Button variant="secondary" size="sm" aria-expanded={openInspector === 'optimize'} aria-haspopup={inspectorIsSheet ? 'dialog' : undefined} aria-controls={openInspector === 'optimize' ? 'workbench-inspector' : undefined} onClick={event => toggleInspector('optimize', event.currentTarget)}><SlidersHorizontal aria-hidden="true" /> Optimize</Button>
                    <Button variant="secondary" size="sm" aria-expanded={openInspector === 'tools'} aria-haspopup={inspectorIsSheet ? 'dialog' : undefined} aria-controls={openInspector === 'tools' ? 'workbench-inspector' : undefined} onClick={event => toggleInspector('tools', event.currentTarget)}><Wrench aria-hidden="true" /> More tools</Button>
                  </>
                )}
                </div>
                </div>
                <ModelPreview asset={previewAsset} comparison={previewComparison} loadModel={loadPreview} onError={reportPreviewError} />
                {showVersionStats && <div className="workbench-comparison-meta" role="group" aria-label="Original to result comparison">
                  {typeof beforeTriangles === 'number' && typeof afterTriangles === 'number' && <span>Triangles <strong>{beforeTriangles.toLocaleString()} → {afterTriangles.toLocaleString()}</strong></span>}
                  {comparisonSizes && <span>GLB size <strong>{comparisonSizes}</strong></span>}
                </div>}
                {selectedAsset && (
                  <div className="asset-meta-strip">
                    <div className="asset-meta-main"><Badge variant="outline" className="file-badge">{selectedAsset.kind === 'mesh' ? 'GLB' : selectedAsset.mime_type === 'image/png' ? 'PNG' : 'JPG'}</Badge><div><strong>{selectedAsset.name}</strong><span>{formatSize(selectedAsset.byte_size)} · {selectedAsset.kind === 'mesh' ? `${Number(selectedAsset.metadata.mesh_count ?? 0)} mesh${Number(selectedAsset.metadata.mesh_count ?? 0) === 1 ? '' : 'es'}` : 'Image asset'}</span></div></div>
                    <div className="meta-divider" />
                    {selectedAsset.kind === 'mesh' ? <><div className="meta-stat"><span>Triangles</span><strong>{triangleCount.toLocaleString()}</strong></div><div className="meta-stat"><span>Materials</span><strong>{Number(selectedAsset.metadata.material_count ?? 0)}</strong></div></> : <div className="meta-stat"><span>Input</span><strong>{selectedAsset.mime_type === 'image/png' ? 'PNG' : 'JPEG'}</strong></div>}
                    <div className="meta-stat"><span>File</span><strong className="status-ready">Imported</strong></div>
                  </div>
                )}
              {reportCard && (
                <details className="result-card workbench-result" aria-label={`${reportCard.title} result`}>
                  <summary>Operation report</summary>
                  <div className="result-heading"><strong>{reportCard.title} complete</strong></div>
                  {!reportCard.reportAvailable && <p className="report-unavailable">The operation completed, but its report payload is missing or unreadable.</p>}
                  <div className="report-metrics">
                    {reportCard.metrics.map((metric) => (
                      <div className="report-metric" key={metric.label}>
                        <span>{metric.label}</span>
                        <strong>{metric.value}</strong>
                      </div>
                    ))}
                  </div>
                  {reportCard.lists.map((list) => (
                    <div className="report-list" key={list.label}>
                      <strong>{list.label}</strong>
                      {list.state === 'unavailable' && <p>Not reported.</p>}
                      {list.state === 'empty' && <p>None reported.</p>}
                      {list.state === 'partial' && <p>Some entries were invalid or omitted.</p>}
                      {list.values.length > 0 && <ul>{list.values.map((value, index) => <li key={`${index}-${value}`}>{value}</li>)}</ul>}
                      {list.omittedCount > 0 && <p>{list.omittedCount.toLocaleString()} additional entries are not shown.</p>}
                    </div>
                  ))}
                  <section className="report-warnings" aria-label="Warnings">
                    <strong>Warnings</strong>
                    {reportCard.warningsState === 'unavailable' && <p>{reportCard.warningsMessage}</p>}
                    {reportCard.warningsState === 'empty' && <p>No warnings reported.</p>}
                    {reportCard.warningFormatIssue && <p>Some warning data was missing or invalid; all readable warning text is listed below.</p>}
                    {reportCard.warnings.length > 0 && <ul>{reportCard.warnings.map((warning, index) => <li key={`${index}-${warning}`}>{warning}</li>)}</ul>}
                  </section>
                  <details className="report-technical">
                    <summary>Asset and job IDs</summary>
                    <dl className="report-lineage">
                      <div><dt>Job</dt><dd><code>{reportCard.jobId}</code></dd></div>
                      <div><dt>Input</dt><dd><code>{reportCard.inputAssetId ?? 'Not reported'}</code></dd></div>
                      <div><dt>Result</dt><dd><code>{reportCard.resultAssetId ?? 'Not reported'}</code></dd></div>
                    </dl>
                  </details>
                </details>
              )}
              </div>}

            </section>

            {openInspector && !inspectorIsSheet && <aside className="settings-column active-inspector workbench-inspector" id="workbench-inspector" data-tool={openInspector}
              aria-labelledby="workbench-inspector-title" onKeyDown={event => {
                if (event.key === 'Escape') { event.stopPropagation(); closeInspector(); }
              }}>
              <div className="workbench-inspector-heading">
                <h2 id="workbench-inspector-title">{inspectorTitle}</h2>
                <Button variant="ghost" size="icon-sm" className="workbench-close" title="Close settings" aria-label="Close settings" onClick={closeInspector}><PanelRightClose aria-hidden="true" /></Button>
              </div>
              {inspectorBody()}
            </aside>}
          </div>
          {showInspectorSheet && <Sheet open={showInspectorSheet} onOpenChange={open => { if (!open) setInspector(null); }}
            title={inspectorTitle} portal={workbenchNode} side="right" id="workbench-inspector"
            className="active-inspector workbench-inspector" onCloseAutoFocus={event => {
              event.preventDefault();
              restoreInspectorFocus();
            }}>
            <div className="workbench-inspector-heading">
              <h2 aria-hidden="true">{inspectorTitle}</h2>
              <SheetClose asChild><Button variant="ghost" size="icon-sm" className="workbench-close" aria-label="Close settings" title="Close settings"><PanelRightClose aria-hidden="true" /></Button></SheetClose>
            </div>
            {feedback}
            {jobFeedback}
            {inspectorBody()}
          </Sheet>}
      </main>
      {dragging && <div className="drop-overlay"><div className="drop-overlay-card"><span className="drop-icon">↓</span><strong>Drop a GLB or source image here</strong><span>Import starts locally on your computer</span></div></div>}

      <DialogContent className="connect-dialog" showCloseButton={false}>
        <div className="connect-heading">
          <DialogTitle>Connect local engine</DialogTitle>
          <DialogClose asChild><Button variant="ghost" size="icon-sm" aria-label="Close"><X /></Button></DialogClose>
        </div>
        <DialogDescription>Studio connects to MeshHarbor on this computer. No token is needed here.</DialogDescription>
        <div className="connect-steps"><code>meshharbor serve</code><span>Run in your terminal and leave it open</span></div>
        {connectionError && <Alert variant="destructive" role="alert" className="dialog-error">
          <CircleAlert aria-hidden="true" />
          <AlertDescription>Engine offline. Start it in your terminal, then retry.</AlertDescription>
          <details><summary>Technical details</summary>{connectionError}</details>
        </Alert>}
        <Button className="connect-button" onClick={() => void connect()} disabled={connection === 'connecting'}>
          {connection === 'connecting' ? 'Connecting…' : 'Retry connection'}
        </Button>
      </DialogContent>
      </div>
    </Dialog>
  );
}

function formatJobTime(timestamp: string): string {
  const date = new Date(Number(timestamp));
  return Number.isNaN(date.getTime()) ? 'time unavailable' : date.toLocaleString();
}

function studioOperationLabel(operation: string): string {
  switch (operation) {
    case 'mesh.simplify': return 'Optimize mesh';
    case 'image.generate': return 'Generate model';
    case 'uv.unwrap': return 'Unwrap UVs';
    case 'runtime.pixal3d.setup': return 'Set up MLX';
    case 'runtime.pixal3d.install_models': return 'Download MLX models';
    case 'runtime.pixal3d.cuda_setup': return 'Set up CUDA';
    case 'runtime.pixal3d.cuda_install_models': return 'Download CUDA models';
    case 'runtime.pixal3d.convert_naf': return 'Convert NAF checkpoint';
    default: return meshOperationOptions.find(tool => tool.id === operation)?.title ?? operation;
  }
}
