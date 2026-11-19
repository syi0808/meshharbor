import { ArrowsClockwiseIcon, CubeIcon, DownloadSimpleIcon, FileImageIcon, GearSixIcon, ImageSquareIcon, ListChecksIcon, MagnifyingGlassIcon, PlusIcon, SidebarSimpleIcon, SlidersHorizontalIcon, SquaresFourIcon, WrenchIcon } from '@phosphor-icons/react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
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
  type AssetImportProgress,
  type AssetPageFilters,
  type JobReportCardModel,
} from './api';
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from '@/components/ui/accordion';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Empty, EmptyDescription, EmptyHeader } from '@/components/ui/empty';
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { Progress } from '@/components/ui/progress';
import { Separator } from '@/components/ui/separator';
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet';
import { SidebarHeader, SidebarMenu, SidebarMenuButton, SidebarMenuItem } from '@/components/ui/sidebar';
import { Slider } from '@/components/ui/slider';
import { Spinner } from '@/components/ui/spinner';
import { Toaster } from '@/components/ui/sonner';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group';
import { ModelPreview } from './ModelPreview';
import { AssetGalleryCard } from './AssetGalleryCard';
import { Disclosure } from './Disclosure';
import { MeshStudy } from './MeshStudy';
import { GenerationParameterFields, generationParameters, readyGenerationProfiles } from './GenerationSettings';
import { buildPosedInput, defaultViews, newView, PosedReferences, SingleReference, type ViewDraft } from './GenerationReferences';
import { notify } from './notify';
import { SettingsPage, settingsSectionFromPath, type RuntimeAction, type SettingsSection } from './SettingsPage';
import { applyStudioTheme, loadStudioTheme } from './theme';
import { useWorkbenchSidebar, WorkbenchLayout } from './WorkbenchSidebar';
import { formatVersionSizes, inspectorForAsset, shouldHighlightExport, WORKBENCH_INSPECTOR_SHEET_QUERY, type Inspector } from './workbench-state';
import type { Artifact, Asset, AssetKind, GenerationViewMode, Job, MeshOperationId, Operation, PosedViewMode, RuntimeBackendId, RuntimeInfo } from './types';

type Connection = 'disconnected' | 'connecting' | 'connected';
type AssetOriginFilter = 'all' | 'original' | 'derived';
type AssetImportPurpose = 'library' | 'reference' | { mode: PosedViewMode; maxViews: number; index?: number };
const viewLabels: Record<GenerationViewMode, string> = { single: '1 view', four: '4 views', six: '6 views', custom: 'Custom views' };
interface AssetBrowserFilters {
  query: string;
  kind: AssetKind;
  origin: AssetOriginFilter;
}

const ASSET_PAGE_SIZE = 50;
const ASSET_GALLERY_GRID = 'grid min-w-0 grid-cols-2 gap-2 @min-[380px]:grid-cols-3';
const EMPTY_ASSET_BROWSER_FILTERS: AssetBrowserFilters = { query: '', kind: 'mesh', origin: 'all' };

function toAssetPageFilters(filters: AssetBrowserFilters): AssetPageFilters {
  const query = filters.query.trim();
  return {
    ...(query ? { query } : {}),
    kind: filters.kind,
    ...(filters.origin === 'all' ? {} : { derived: filters.origin === 'derived' }),
  };
}

function matchesAssetBrowserFilters(asset: Asset, filters: AssetBrowserFilters): boolean {
  return (!filters.query || asset.name.toLowerCase().includes(filters.query.toLowerCase()))
    && asset.kind === filters.kind
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

const meshOperationOptions: Array<{ id: MeshOperationId; title: string; description: string }> = [
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
    description: 'Find connected surfaces across exact seams without welding. On fragmented meshes, small components share one primitive to stay within the output limit.',
  },
  {
    id: 'uv.unwrap',
    title: 'UV atlas rebake',
    description: 'Repack one textured mesh primitive with one material and rebake its embedded base-color and metallic/roughness PNGs. Zero-area faces are omitted from the result. This is not a high-to-low bake.',
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
  const [theme, setTheme] = useState(loadStudioTheme);
  useLayoutEffect(() => { applyStudioTheme(theme); }, [theme]);
  const [connection, setConnection] = useState<Connection>('disconnected');
  const [settingsSection, setSettingsSection] = useState(() => settingsSectionFromPath(window.location.pathname));
  const [settingsBackend, setSettingsBackend] = useState<RuntimeBackendId | null>(null);
  const settingsTrigger = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    function onPopState() {
      setSettingsSection(settingsSectionFromPath(window.location.pathname));
    }
    window.addEventListener('popstate', onPopState);
    return () => window.removeEventListener('popstate', onPopState);
  }, []);
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
  const [targetQuads, setTargetQuads] = useState('10000');
  const requestedQuads = Number(targetQuads);
  const validQuadTarget = Number.isInteger(requestedQuads) && requestedQuads >= 4 && requestedQuads <= 50000;
  const [maxError, setMaxError] = useState(0.02);
  const [generationModelId, setGenerationModelId] = useState('');
  const [generationDrafts, setGenerationDrafts] = useState<Record<string, Record<string, string>>>({});
  const [generationSource, setGenerationSource] = useState<Asset | null>(null);
  const [preferredViewMode, setPreferredViewMode] = useState<GenerationViewMode>('single');
  const [posedDrafts, setPosedDrafts] = useState<Partial<Record<PosedViewMode, ViewDraft[]>>>({});
  const [generationResult, setGenerationResult] = useState<Job | null>(null);
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
  const [selectedTool, setSelectedTool] = useState<Inspector>('generation');
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
  const workbenchRef = useRef<HTMLElement>(null);
  const [dragging, setDragging] = useState(false);
  const [importProgress, setImportProgress] = useState<AssetImportProgress | null>(null);
  const [jobPollingIssue, setJobPollingIssue] = useState<{ jobId: string; message: string } | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const inventoryInputRef = useRef<HTMLInputElement>(null);
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
  const pinnedAssets = assets.filter(asset => asset.kind === assetFilters.kind && pinnedAssetIds.has(asset.id) && !matchingAssetIds.has(asset.id));
  const hasActiveAssetFilters = assetFilters.query !== ''
    || assetFilters.origin !== 'all';
  const showLibraryFilters = assets.length > 0 || hasActiveAssetFilters;
  const quadArtifact = quadTopologyArtifact(selectedAsset);
  const quadriflowBackend = runtime?.quadriflow_backend;
  const [checkingRetopology, setCheckingRetopology] = useState(false);
  const [retopologyCheck, setRetopologyCheck] = useState<{ assetId: string; message: string } | null>(null);
  const [installingBackend, setInstallingBackend] = useState(false);
  const isRunning = isActiveJob(job);
  const backendStatus = (id: string) => runtime?.model_backends.find((backend) => backend.id === id)?.status.replaceAll('_', ' ') ?? 'Connect engine';
  const generationProfiles = useMemo(() => readyGenerationProfiles(runtime), [runtime]);
  const selectedGenerationProfile = generationProfiles.find(profile => profile.modelId === generationModelId) ?? generationProfiles[0];
  const viewMode = selectedGenerationProfile?.multiviewReady && selectedGenerationProfile.view_modes.includes(preferredViewMode)
    ? preferredViewMode : 'single';
  const posedViews = viewMode === 'single' ? [] : posedDrafts[viewMode] ?? defaultViews(viewMode);
  const posedInput = viewMode === 'single' ? null : buildPosedInput(viewMode, posedViews, selectedGenerationProfile?.max_views ?? 1);
  const generationAssetId = viewMode === 'single' ? generationSource?.id : posedInput?.references[0].asset_id;
  const generationDraft = selectedGenerationProfile ? generationDrafts[selectedGenerationProfile.id] ?? {} : {};
  const generationPayload = selectedGenerationProfile
    ? generationParameters(selectedGenerationProfile, generationDraft, viewMode !== 'single') : null;
  function openSettings(section: SettingsSection, backendId?: RuntimeBackendId) {
    if (backendId) setSettingsBackend(backendId);
    if (inspectorIsSheet) setInspector(null);
    if (sidebar.narrow && sidebar.open) sidebar.setOpen(false);
    const path = `/settings/${section}`;
    if (window.location.pathname !== path) window.history.pushState(null, '', path);
    setSettingsSection(section);
  }

  function closeSettings() {
    window.history.pushState(null, '', '/');
    setSettingsSection(null);
    requestAnimationFrame(() => settingsTrigger.current?.focus());
  }

  async function requestAssetPage(
    filters: AssetBrowserFilters,
    append: boolean,
  ): Promise<string | null | undefined> {
    if (connection !== 'connected') return;
    const cursor = append ? assetNextCursor : null;
    if (append && !cursor) return;

    const requestId = assetPageRequestGuard.current.begin();
    assetFiltersRef.current = filters;
    setAssetFilters(filters);
    setAssetPageLoading(true);
    if (!append) setAssetNextCursor(null);
    try {
      const page = await api.assetPage(ASSET_PAGE_SIZE, cursor, toAssetPageFilters(filters));
      if (!assetPageRequestGuard.current.isCurrent(requestId)) return;
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
      if (!assetPageRequestGuard.current.isCurrent(requestId)) return;
      return cause instanceof Error
        ? 'Could not load the asset library. ' + cause.message
        : 'Could not load the asset library.';
    } finally {
      if (assetPageRequestGuard.current.isCurrent(requestId)) setAssetPageLoading(false);
    }
  }

  async function refreshAssets(append = false) {
    const result = await requestAssetPage(assetFilters, append);
    if (result === null) notify('success', append ? 'More assets loaded.' : 'Asset library refreshed.');
    else if (result) notify('error', result);
  }

  function applyAssetBrowserFilters(filters: AssetBrowserFilters) {
    const normalized = { ...filters, query: clampAssetSearchQuery(filters.query) };
    assetBrowserInteractionRevision.current += 1;
    assetSearchInputRef.current = normalized.query;
    setAssetSearchInput(normalized.query);
    if (connection !== 'connected') {
      assetFiltersRef.current = normalized;
      setAssetFilters(normalized);
      return;
    }
    void requestAssetPage(normalized, false).then(problem => { if (problem) notify('error', problem); });
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
      if (!recentJobsRequestGuard.current.isCurrent(requestId)) return false;
      setRecentJobs(jobs);
      return true;
    } catch (cause) {
      if (!recentJobsRequestGuard.current.isCurrent(requestId)) return false;
      throw new Error(cause instanceof Error ? `Could not refresh recent job records. ${cause.message}` : 'Could not refresh recent job records.');
    }
  }

  async function refreshJobHistory() {
    try {
      if (await refreshRecentJobs()) notify('success', 'Job history refreshed.');
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not refresh job history.');
    }
  }

  async function openRecentJob(jobId: string) {
    const requestId = historySelectionGuard.current.begin();
    setComparisonLoadingSide(null);
    setReportJob(null);
    try {
      const response = await api.job(jobId);
      if (!historySelectionGuard.current.isCurrent(requestId)) return;
      if (shouldDeferTrackedJobCompletion(jobRef.current, response)) {
        recentJobsRequestGuard.current.invalidate();
        setRecentJobs((current) => [response, ...current.filter((record) => record.id !== response.id)]
          .sort((left, right) => Number(right.updated_at) - Number(left.updated_at)
            || right.id.localeCompare(left.id))
          .slice(0, 20));
        notify('info', 'The live status monitor will apply this result.');
        return;
      }
      const latest = preferTrackedTerminalJob(jobRef.current, response);
      if (!canInspectJobHistoryRecord(jobRef.current, latest)) {
        notify('warning', 'Wait for the active job to finish before opening a different saved result.');
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
          notify('warning', 'A new job is active. Try viewing the saved result after it finishes.');
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
        notify('info', jobRef.current?.id === latest.id
          ? 'The current job is still being monitored.'
          : `Last saved state: ${latest.state} (${latest.progress}%). No work was resumed.`);
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
        notify('error', latest.error?.message ?? 'This saved job failed.');
      } else {
        notify('info', `Loaded the saved ${studioOperationLabel(latest.operation)} record.`);
      }
    } catch (cause) {
      if (!historySelectionGuard.current.isCurrent(requestId)) return;
      notify('error', cause instanceof Error ? cause.message : 'Could not load this saved job.');
    }
  }

  const installQuadriflowBackend = useCallback(async () => {
    if (!quadriflowBackend?.install_available || installingBackend) return;
    setInstallingBackend(true);
    try {
      await api.installQuadriflowBackend();
      const [runtimeInfo, operationList] = await Promise.all([api.runtime(), api.operations()]);
      setRuntime(runtimeInfo);
      setOperations(operationList);
      notify('success', 'QuadriFlow backend installation completed.');
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'QuadriFlow backend installation failed.');
    } finally {
      setInstallingBackend(false);
    }
  }, [api, installingBackend, quadriflowBackend?.install_available]);

  const connect = useCallback(async (announce = false) => {
    historySelectionGuard.current.invalidate();
    assetPageRequestGuard.current.invalidate();
    setAssetPageLoading(true);
    setComparisonLoadingSide(null);
    setComparisonPair(null);
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
      if (announce) notify('success', 'Connected to the local engine.');
    } catch (cause) {
      setAssetPageLoading(false);
      setConnection('disconnected');
      const message = cause instanceof Error ? cause.message : 'Could not connect to MeshHarbor.';
      setConnectionError(message);
      if (announce) notify('error', `Could not connect to the local engine. ${message}`);
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
        setJobPollingIssue(null);
        if (latest.state === 'succeeded') {
          if (latest.operation.startsWith('runtime.')) {
            let refreshed: [RuntimeInfo, Operation[]] | null = null;
            try {
              refreshed = await Promise.all([api.runtime(), api.operations()]);
            } catch (cause) {
              if (!stopped) {
                setJob(latest);
                notify('warning', cause instanceof Error ? `Runtime setup finished, but status could not be refreshed. ${cause.message}` : 'Runtime setup finished, but status could not be refreshed.');
              }
              return;
            }
            if (stopped || !refreshed) return;
            const [runtimeInfo, operationList] = refreshed;
            setJob(latest);
            setReportJob(null);
            setRuntime(runtimeInfo);
            setOperations(operationList);
            notify('success', latest.operation === 'runtime.pixal3d.setup'
              ? 'The isolated Python environment is ready. Connect Hugging Face in Settings before downloading model files.'
              : latest.operation === 'runtime.pixal3d.install_models'
                ? 'Pinned model files are ready. MeshHarbor completed the one-time NAF conversion in an isolated tool environment.'
                : latest.operation === 'runtime.pixal3d.cuda_setup'
                  ? 'The isolated CUDA environment and GPU preflight are ready. Connect Hugging Face in Settings before downloading model files.'
                  : latest.operation === 'runtime.pixal3d.cuda_install_multiview'
                    ? 'Pixal3D multiview weights are ready.'
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
          if (derived) {
            const completedAsset = derived;
            const pair = assetComparisonPairFromJob(latest, completedAsset);
            const isGeneration = latest.operation === 'image.generate';
            const advance = shouldAutoSelectComparisonResult(pair, selectedIdRef.current)
              || (isGeneration && selectedIdRef.current === null);
            if (!isGeneration || advance) {
              setComparisonPair(pair);
              setReportJob(latest);
            }
            if (isGeneration) setGenerationResult(latest);
            setAssets((current) => [completedAsset, ...current.filter((item) => item.id !== completedAsset.id)]);
            if (advance) {
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
            notify('success', removed > 0
              ? `Cleanup complete. Removed ${removed.toLocaleString()} invalid or duplicate triangles.`
              : 'Cleanup complete. No removable triangles were found.');
          } else if (latest.operation === 'render.optimize') {
            const before = Number(resultReport.vertex_cache_misses_before ?? 0);
            const after = Number(resultReport.vertex_cache_misses_after ?? 0);
            notify('success', `Render optimization complete. Estimated vertex-cache misses: ${before.toLocaleString()} → ${after.toLocaleString()}.`);
          } else if (latest.operation === 'mesh.compress') {
            const saved = Number(resultReport.bytes_saved ?? 0);
            notify('success', `Compression complete. Saved ${formatSize(saved)}. The exported GLB requires KHR_meshopt_compression support.`);
          } else if (latest.operation === 'mesh.separate_parts') {
            const parts = Number(resultReport.connected_parts ?? 0);
            const grouped = Number(resultReport.grouped_components ?? 0);
            const primitives = Number(resultReport.output_primitives ?? parts);
            notify('success', grouped > 0
              ? `Part separation complete. Found ${parts.toLocaleString()} connected components. ${grouped.toLocaleString()} small components share the last of ${primitives.toLocaleString()} output primitives; no triangles were discarded.`
              : `Part separation complete. Found ${parts.toLocaleString()} connected component${parts === 1 ? '' : 's'}, emitted as primitives in the same mesh.`);
          } else if (latest.operation === 'mesh.retopology') {
            const projection = resultReport.appearance_projection;
            const coverage = projection && typeof projection === 'object'
              ? Number((projection as Record<string, unknown>).projection_coverage)
              : Number.NaN;
            const projectionNotice = Number.isFinite(coverage)
              ? ` Supported base-color and metallic-roughness maps passed ${(coverage * 100).toFixed(1)}% projection coverage in the preview.`
              : ' No appearance maps were projected.';
            notify('success', `Experimental retopology complete.${projectionNotice}`);
          } else if (latest.operation === 'uv.unwrap') {
            const width = Number(resultReport.atlas_width ?? 0);
            const height = Number(resultReport.atlas_height ?? 0);
            const size = width && height ? ` at ${width}×${height}px` : '';
            notify('success', `Atlas UV unwrap and same-mesh texture rebake complete${size}.`);
          } else {
            notify('success', latest.operation === 'image.generate'
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
          notify('error', latest.error?.message ?? (latest.operation === 'image.generate'
            ? 'This image generation job failed.'
            : latest.operation === 'mesh.simplify'
              ? 'This simplification job failed.'
              : 'This mesh processing job failed.'));
          return;
        }
        if (latest.state === 'cancelled') {
          setJob(latest);
          notify('success', 'The queued job was cancelled.');
          return;
        }
        setJob(latest);
        timer = window.setTimeout(poll, 700);
      } catch (cause) {
        if (!stopped) {
          consecutiveFailures += 1;
          const delay = jobPollRetryDelay(consecutiveFailures);
          const detail = cause instanceof Error ? cause.message : 'The local engine could not be reached.';
          setJobPollingIssue({ jobId: job.id, message: `Could not check job status. ${detail} Retrying in ${Math.ceil(delay / 1000)}s.` });
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

  const loadPreview = useCallback((asset: Asset, signal?: AbortSignal) => {
    if (connection !== 'connected') return Promise.reject(new Error('The local MeshHarbor engine is offline.'));
    return api.artifact(asset, signal);
  }, [api, connection]);
  const reportPreviewError = useCallback((message: string) => notify('error', `Preview: ${message}`), []);

  function selectAsset(asset: Asset) {
    if (asset.kind !== 'mesh') setInspector(current => inspectorForAsset(current, asset.kind));
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
    try {
      const asset = await resolveComparisonAsset(pair, side, assets, (assetId) => api.derivedAsset(assetId));
      if (!historySelectionGuard.current.isCurrent(requestId)) return;
      setAssets((current) => [asset, ...current.filter((item) => item.id !== asset.id)]);
      selectAssetId(asset.id);
    } catch (cause) {
      if (!historySelectionGuard.current.isCurrent(requestId)) return;
      const label = side === 'source' ? 'original' : 'result';
      notify('error', cause instanceof Error ? `Could not load the comparison ${label}. ${cause.message}` : `Could not load the comparison ${label}.`);
    } finally {
      if (historySelectionGuard.current.isCurrent(requestId)) setComparisonLoadingSide(null);
    }
  }

  async function importFiles(files: FileList | File[], purpose: AssetImportPurpose = 'library') {
    if (files.length === 0) return;
    if (importBatchInFlight.current) {
      notify('warning', 'An import batch is already running. These additional files were not queued.');
      return;
    }
    if (busy) {
      notify('warning', 'Wait for the current Studio operation to finish before importing files.');
      return;
    }
    if (connection !== 'connected') {
      openSettings('models');
      notify('warning', 'Start the local engine before importing files.');
      return;
    }
    if (purpose !== 'library' && Array.from(files).some(file => !/\.(png|jpe?g)$/i.test(file.name))) {
      notify('warning', 'Generation references must be PNG or JPEG images.');
      return;
    }
    if (typeof purpose === 'object' && (purpose.index !== undefined ? files.length !== 1 : files.length < 2 || files.length > purpose.maxViews)) {
      notify('warning', purpose.index === undefined ? `Choose 2–${purpose.maxViews} images.` : 'Choose one image for this view.');
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
    setBusy(true);
    try {
      const result = await importAssetBatch(files, async (file, kind) =>
        kind === 'image' ? api.importImage(file) : api.importGlb(file), setImportProgress);
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
        if (purpose === 'reference') {
          const reference = result.assets.find(asset => asset.kind === 'image');
          if (reference) setGenerationSource(reference);
        } else if (typeof purpose === 'object') {
          const images = result.assets.filter(asset => asset.kind === 'image');
          if (images.length === result.attemptedCount && result.failures.length === 0) {
            setPosedDrafts(previous => {
              const drafts = previous[purpose.mode] ?? defaultViews(purpose.mode);
              const next = purpose.index === undefined
                ? images.map((image, index) => ({ ...(drafts[index] ?? newView(index)), image }))
                : drafts.map((view, index) => index === purpose.index ? { ...view, image: images[0] } : view);
              return { ...previous, [purpose.mode]: next };
            });
          } else {
            notify('warning', 'The reference mapping was kept unchanged because not every image imported successfully.');
          }
        } else if (plan.selectFirstImportedAsset) {
          invalidateHistoryRequests();
          comparisonPairRef.current = null;
          setComparisonPair(null);
          if (result.assets[0].kind !== 'mesh') setInspector(current => inspectorForAsset(current, result.assets[0].kind));
          selectAssetId(result.assets[0].id);
          setReportJob(null);
        }

        if (purpose === 'library' && plan.resetFilters) {
          filtersToRefresh = { ...EMPTY_ASSET_BROWSER_FILTERS, kind: current.filters.kind };
          assetSearchInputRef.current = '';
          setAssetSearchInput('');
        }
      }
      if (result.attemptedCount > 0) {
        // Reconcile ambiguous failures where the server may have stored an asset
        // before the response was lost, without ever replaying the upload.
        refreshError = await requestAssetPage(filtersToRefresh, false) ?? null;
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
        notify(result.assets.length > 0 ? 'warning' : 'error', summary);
      } else {
        if (summary) notify('success', summary);
      }
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not import these files.');
    } finally {
      importBatchInFlight.current = false;
      setImportProgress(null);
      setBusy(false);
    }
  }

  async function simplify() {
    if (!selectedAsset) return;
    invalidateHistoryRequests();
    setBusy(true);
    setReportJob(null);
    setComparisonPair(null);
    try {
      const nextJob = await api.createSimplifyJob(selectedAsset.id, Math.max(1, Math.floor(targetTriangles)), maxError);
      setJob(nextJob);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not start simplification.');
    } finally {
      setBusy(false);
    }
  }

  async function checkRetopologyInput() {
    if (!selectedAsset || selectedAsset.kind !== 'mesh' || checkingRetopology || !validQuadTarget) return;
    const assetId = selectedAsset.id;
    setCheckingRetopology(true);
    setRetopologyCheck(null);
    try {
      const report = await api.preflightRetopology(assetId, requestedQuads);
      const input = report.appearance_projection_required ? 'Textured input' : 'Geometry-only input';
      const seams = report.coincident_boundary_vertices_joined ? ' Exact seams can be joined for the solver without changing the source.' : '';
      setRetopologyCheck({ assetId, message: `${input} passed the input checks (${report.input_triangles.toLocaleString()} triangles).${seams} The solver was not run.` });
      notify('success', 'Retopology input checks passed.');
    } catch (cause) {
      setRetopologyCheck({ assetId, message: cause instanceof Error ? cause.message : 'Could not check the retopology input.' });
      notify('error', 'Retopology input checks failed.');
    } finally {
      setCheckingRetopology(false);
    }
  }

  async function runMeshOperation(operation: MeshOperationId) {
    if (!selectedAsset || selectedAsset.kind !== 'mesh' || (operation === 'mesh.retopology' && !validQuadTarget)) return;
    invalidateHistoryRequests();
    setBusy(true);
    setReportJob(null);
    setComparisonPair(null);
    try {
      const parameters: Record<string, number> = {};
      if (operation === 'render.optimize') parameters.overdraw_threshold = 1.05;
      if (operation === 'mesh.retopology') parameters.target_quads = requestedQuads;
      const nextJob = await api.createMeshOperationJob(selectedAsset.id, operation, parameters);
      setJob(nextJob);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not start mesh processing.');
    } finally {
      setBusy(false);
    }
  }

  async function generate() {
    if (!generationAssetId || !selectedGenerationProfile || !generationPayload || !canGenerate || busy || isRunning || connection !== 'connected') return;
    invalidateHistoryRequests();
    setBusy(true);
    try {
      const nextJob = await api.createImageGenerationJob(generationAssetId, selectedGenerationProfile.id, {
        ...generationPayload,
        ...(posedInput ? { view_input: posedInput } : {}),
      });
      setGenerationResult(null);
      setJob(nextJob);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not start image generation.');
    } finally {
      setBusy(false);
    }
  }

  async function runRuntimeAction(backend: RuntimeBackendId, action: RuntimeAction) {
    if (busy || isRunning || connection !== 'connected') return;
    invalidateHistoryRequests();
    setBusy(true);
    try {
      const cuda = backend === 'pixal3d-cuda';
      const nextJob = action === 'multiview' ? await api.installPixal3dMultiview() : action === 'setup'
        ? await (cuda ? api.setupPixal3dCudaRuntime() : api.setupPixal3dRuntime())
        : action === 'download'
          ? await (cuda ? api.installPixal3dCudaModels() : api.installPixal3dModels())
          : await api.convertPixal3dNaf();
      setJob(nextJob);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not start the runtime operation.');
    } finally {
      setBusy(false);
    }
  }

  async function downloadResult(artifact?: Artifact) {
    if (!selectedAsset || downloadInFlight.current || connection !== 'connected') return;
    downloadInFlight.current = true;
    setDownloading(true);
    try {
      const blob = artifact ? await api.downloadArtifact(artifact) : await api.artifact(selectedAsset);
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = artifact ? `${selectedAsset.name.replace(/\.[^.]+$/, '')}-quads.obj` : selectedAsset.name;
      anchor.click();
      notify('success', `Export started: ${anchor.download}`);
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not download this asset.');
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
      notify(updated.state === 'cancelled' ? 'success' : 'warning', cancelJobNotice(updated));
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not cancel this job.');
    }
  }

  const triangleCount = Number(selectedAsset?.metadata.triangles ?? 0);
  const canSimplify = !!selectedAsset && selectedAsset.kind === 'mesh' && !!operations.find((operation) => operation.id === 'mesh.simplify')?.implemented;
  const canGenerate = !!operations.find((operation) => operation.id === 'image.generate')?.implemented
    && selectedGenerationProfile !== undefined;
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
  const activeTool = inspectorForAsset(selectedTool, selectedAsset?.kind) ?? 'generation';
  function selectInspector(next: Inspector, trigger: HTMLButtonElement) {
    if (openInspector === next) return;
    inspectorTrigger.current = trigger;
    setSelectedTool(next);
    setInspector(next);
  }
  const inspectorTitle = openInspector === 'generation' ? 'Generate model' : openInspector === 'optimize' ? 'Optimize mesh' : openInspector === 'jobs' ? 'Jobs' : 'Mesh tools';
  const closeInspectorLabel = openInspector === 'jobs' ? 'Close jobs' : 'Close settings';
  const showInspectorSheet = inspectorIsSheet && !!openInspector;
  const exportIsPrimary = shouldHighlightExport(activeView, openInspector, isRunning, connection === 'connected');
  const jobShowsProgress = isRunning && displaysJobProgress(job.operation);
  function restoreInspectorFocus() {
    const trigger = inspectorTrigger.current;
    if (trigger?.isConnected && trigger.getClientRects().length) trigger.focus();
    else workbenchRef.current?.querySelector<HTMLButtonElement>('[data-slot="model-tools"] button')?.focus();
  }
  function closeInspector() {
    setInspector(null);
    restoreInspectorFocus();
  }
  const jobFeedback = isRunning ? (
    <div className="grid gap-2 rounded-lg border bg-card p-3 text-xs text-card-foreground">
      <div className="flex flex-wrap items-center gap-2" role="status">
        {jobShowsProgress
          ? <Spinner aria-hidden="true" />
          : <Spinner role="progressbar" aria-label="Job progress" />}
        <strong>{job.state === 'queued' ? 'Queued' : job.state === 'validating' ? 'Validating' : job.state === 'cancelling' ? 'Cancelling' : 'Working'}</strong>
        <span>{studioOperationLabel(job.operation)}{jobShowsProgress && ` · ${job.progress}%`}</span>
      </div>
      {jobShowsProgress && <Progress value={job.progress} aria-label="Job progress" />}
      {jobPollingIssue?.jobId === job.id && <p className="wrap-anywhere text-muted-foreground">{jobPollingIssue.message}</p>}
      {job.operation === 'image.generate' && job.state === 'running' && <p className="text-muted-foreground">Running locally. Generation can take several minutes.</p>}
      {isQueuedJobCancellable(job) && <Button variant="outline" size="sm" className="justify-self-start" onClick={() => void cancelJob()}>Cancel queued job</Button>}
    </div>
  ) : null;
  const workbenchActivity = jobFeedback || importProgress ? <>
    {jobFeedback}
    {importProgress && <div className="flex items-start gap-2 rounded-lg border bg-card p-3 text-xs" role="status">
      <Spinner aria-hidden="true" className="shrink-0" />
      <span className="min-w-0 wrap-anywhere">Importing {importProgress.current} of {importProgress.total}: {importProgress.fileName}</span>
    </div>}
  </> : null;

  function renderAssetCard(asset: Asset) {
    return <AssetGalleryCard key={asset.id} asset={asset} loadPreview={loadPreview} selected={asset.id === selectedId}
      onSelect={() => { selectAsset(asset); if (sidebar.narrow) sidebar.setOpen(false); }} />;
  }

  function inspectorBody() {
    if (openInspector === 'jobs') return (
        <section className="grid min-w-0 grid-cols-1 gap-3 p-3 text-xs" aria-label="Recent jobs">
          <div className="flex min-w-0 items-center justify-between gap-2 text-muted-foreground">
            <span>Recent activity</span>
            <Button variant="ghost" size="sm" onClick={() => void refreshJobHistory()} disabled={connection !== 'connected'}>Refresh <ArrowsClockwiseIcon aria-hidden="true" /></Button>
          </div>
          {recentJobs.some(savedJob => isActiveJob(savedJob) && savedJob.id !== job?.id) &&
            <Disclosure className="rounded-lg border p-2 wrap-anywhere text-muted-foreground" title="Recover an interrupted job">
              <p>Checking a record does not resume it. If the engine exited unexpectedly, stop all REST/MCP processes for this data directory before running <code>meshharbor jobs recover-interrupted --confirm-no-running-meshharbor</code>.</p>
            </Disclosure>}
          {recentJobs.length === 0
            ? <Empty><EmptyHeader><EmptyDescription>No saved jobs yet.</EmptyDescription></EmptyHeader></Empty>
            : <div className="grid gap-2">{recentJobs.slice(0, 8).map((savedJob) => (
              <div className="flex min-w-0 flex-wrap items-center gap-2 rounded-md border p-2 data-[selected=true]:bg-muted" data-selected={savedJob.id === job?.id} key={savedJob.id}>
                <div className="grid min-w-0 flex-1 gap-1 wrap-anywhere [&>strong]:font-medium [&>small]:text-muted-foreground"><strong>{studioOperationLabel(savedJob.operation)}</strong><small>Updated {formatJobTime(savedJob.updated_at)}</small></div>
                <div className="shrink-0 text-right text-xs tabular-nums text-muted-foreground data-[state=failed]:text-destructive [&>strong]:block" data-state={savedJob.state}><strong>{savedJob.state === 'succeeded' ? 'Complete' : savedJob.state[0].toUpperCase() + savedJob.state.slice(1)}</strong>{isActiveJob(savedJob) && displaysJobProgress(savedJob.operation) && <span>{savedJob.progress}%</span>}</div>
                <Button variant="ghost" size="sm" onClick={() => { void openRecentJob(savedJob.id); if (inspectorIsSheet) setInspector(null); }} disabled={connection !== 'connected' || (isRunning && savedJob.id !== job?.id)}>{['queued', 'running', 'validating', 'cancelling'].includes(savedJob.state) ? 'Check' : 'View'}</Button>
              </div>
            ))}</div>}
        </section>
    );
    return <>
      {(openInspector === 'optimize' || openInspector === 'tools') && selectedAsset?.kind === 'mesh' && (
        <div className="grid min-w-0 gap-1 border-b px-4 py-3 text-xs">
          <span className="text-muted-foreground">Selected model</span>
          <span className="truncate font-medium" title={selectedAsset.name}>{selectedAsset.name}</span>
        </div>
      )}
      {openInspector === 'optimize' && <div className="grid gap-4 p-4 text-xs/relaxed">
        <p className="text-muted-foreground">Reduce polygon count while keeping the original file intact and preserving material references.</p>
        <Field>
          <FieldLabel htmlFor="triangle-target">Triangle target</FieldLabel>
          <Input id="triangle-target" type="number" min="1" max="10000000" step="1" value={targetTriangles} onChange={(event) => setTargetTriangles(Number(event.target.value))} disabled={!selectedAsset || isRunning || busy} />
          <FieldDescription>Goal, not a guarantee</FieldDescription>
        </Field>
        <div className="flex items-center gap-2 text-muted-foreground [&>span]:shrink-0">
          <span>Smaller</span>
          <Slider className="min-w-0 flex-1" aria-label="Triangle target" min={1} max={Math.max(4, triangleCount || 20000)} step={1} value={[Math.max(1, Math.min(targetTriangles, Math.max(4, triangleCount || 20000)))]} onValueChange={([value]) => setTargetTriangles(value)} disabled={!selectedAsset || isRunning || busy} />
          <span>More detail</span>
        </div>
        <Accordion type="single" collapsible>
          <AccordionItem value="shape-protection">
            <AccordionTrigger>Shape protection</AccordionTrigger>
            <AccordionContent>
              <div className="grid gap-3">
                <div className="flex items-center justify-between gap-2"><span>Error tolerance</span><span className="font-mono tabular-nums">{maxError.toFixed(3)}</span></div>
                <Slider aria-label="Maximum geometric error" min={0.001} max={0.08} step={0.001} value={[maxError]} onValueChange={([value]) => setMaxError(value)} disabled={!selectedAsset || isRunning || busy} />
                <p className="text-muted-foreground">A lower tolerance may stop above your target to protect shape.</p>
              </div>
            </AccordionContent>
          </AccordionItem>
        </Accordion>
        <div className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-2 gap-y-1 text-muted-foreground"><span className="text-primary">✓</span><span>Keeps the original file</span><span className="text-primary">✓</span><span>Keeps UV and material references</span></div>
        {!(isRunning && job.operation === 'mesh.simplify') && <Button className="w-full" disabled={!canSimplify || busy || connection !== 'connected' || isRunning} onClick={() => void simplify()}>{busy ? <><Spinner aria-hidden="true" /> Working…</> : <><SlidersHorizontalIcon aria-hidden="true" /> Optimize mesh</>}</Button>}
      </div>}

      {openInspector === 'generation' && <section className="grid gap-4 p-4 text-xs/relaxed" id="image-generation">
          {selectedGenerationProfile && <>
            <Field>
              <FieldLabel htmlFor="generation-model">Generation model</FieldLabel>
              <NativeSelect className="w-full" id="generation-model" value={selectedGenerationProfile.modelId}
                onChange={event => { setGenerationModelId(event.target.value); setPreferredViewMode('single'); }} disabled={busy || isRunning}>
                {generationProfiles.map(profile => <NativeSelectOption key={profile.modelId} value={profile.modelId}>{profile.label}</NativeSelectOption>)}
              </NativeSelect>
              <FieldDescription>Only ready local models are listed.</FieldDescription>
            </Field>
            {selectedGenerationProfile.view_modes.length > 1 && <Field>
              <FieldLabel htmlFor="generation-view-mode">View mode</FieldLabel>
              <NativeSelect className="w-full" id="generation-view-mode" value={viewMode} disabled={busy || isRunning}
                onChange={event => {
                  const next = selectedGenerationProfile.view_modes.find(mode => mode === event.target.value);
                  if (next) setPreferredViewMode(next);
                }}>
                {selectedGenerationProfile.view_modes.map(mode => <NativeSelectOption key={mode} value={mode}
                  disabled={mode !== 'single' && !selectedGenerationProfile.multiviewReady}>{viewLabels[mode]}</NativeSelectOption>)}
              </NativeSelect>
              {!selectedGenerationProfile.multiviewReady && <>
                <FieldDescription>Install the additional weights to use multiple reference views.</FieldDescription>
                <Button variant="outline" size="sm" className="self-start" onClick={() => openSettings('storage', 'pixal3d-cuda')}>Set up multiview</Button>
              </>}
            </Field>}
            {viewMode === 'single'
              ? <SingleReference image={generationSource} load={loadPreview} disabled={busy || isRunning}
                onPick={files => void importFiles(files, 'reference')} onClear={() => setGenerationSource(null)} />
              : <PosedReferences mode={viewMode} views={posedViews} maxViews={selectedGenerationProfile.max_views} load={loadPreview} disabled={busy || isRunning}
                onPick={(files, index) => void importFiles(files, { mode: viewMode, maxViews: selectedGenerationProfile.max_views, index })}
                onChange={views => setPosedDrafts(previous => ({ ...previous, [viewMode]: views }))} />}
            <section className="grid gap-3" aria-label="Model settings">
              <h3 className="font-medium">Model settings</h3>
              <GenerationParameterFields profile={selectedGenerationProfile} draft={generationDraft} posed={viewMode !== 'single'} disabled={busy || isRunning}
                onChange={(id, value) => setGenerationDrafts(current => ({
                  ...current,
                  [selectedGenerationProfile.id]: { ...current[selectedGenerationProfile.id], [id]: value },
                }))} />
            </section>
          </>}
          {canGenerate && !(isRunning && job.operation === 'image.generate') && <div className="sticky bottom-0 -mx-4 -mb-4 border-t bg-sidebar p-4">
            <Button className="w-full" disabled={!generationAssetId || !generationPayload || busy || connection !== 'connected' || isRunning}
              onClick={() => void generate()}>{busy ? <><Spinner aria-hidden="true" /> Working…</> : 'Generate 3D'}</Button>
          </div>}
          {!canGenerate && <div className="grid gap-2 text-muted-foreground">
            <p>No generation model is ready. You can still open and edit existing GLBs.</p>
            <Button variant="secondary" onClick={() => openSettings('models')}>Open model setup</Button>
          </div>}
          {generationResult?.result?.asset_id && <Button variant="secondary" className="w-full" disabled={busy || isRunning} onClick={() => {
            closeInspector();
            void openRecentJob(generationResult.id);
          }}>View generated model</Button>}
        </section>}

      {openInspector === 'tools' && <>
      <div className="grid gap-4 p-4 text-xs/relaxed" id="mesh-tools">
        <p className="text-muted-foreground">Mesh processing creates a new asset. Input checks are read-only. Your source file is never overwritten.</p>
        <Accordion type="single" collapsible>
          {meshOperationOptions.map((tool) => {
            const available = operations.find((operation) => operation.id === tool.id)?.implemented === true;
            const processing = isRunning && job.operation === tool.id;
            return (
              <AccordionItem key={tool.id} value={tool.id}>
                <AccordionTrigger>{tool.title}</AccordionTrigger>
                <AccordionContent>
                <div className="grid gap-2">
                <p className="text-muted-foreground">{tool.description}</p>
                {tool.id === 'mesh.retopology' && (
                  <>
                    <Field>
                      <FieldLabel htmlFor="quad-target">Target quads</FieldLabel>
                      <Input id="quad-target" type="number" min="4" max="50000" step="1"
                        value={targetQuads} aria-invalid={!validQuadTarget}
                        onChange={(event) => { setTargetQuads(event.currentTarget.value); setRetopologyCheck(null); }}
                        disabled={busy || isRunning || checkingRetopology} />
                      <FieldDescription>4–50,000. This is a target, not a guaranteed count.</FieldDescription>
                    </Field>
                    <Button
                      variant="secondary" size="sm" className="w-full"
                      disabled={!selectedAsset || selectedAsset.kind !== 'mesh' || !validQuadTarget || busy || checkingRetopology || connection !== 'connected' || isRunning}
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
                    className="w-full"
                    disabled={!selectedAsset || selectedAsset.kind !== 'mesh' || !available || busy || checkingRetopology || connection !== 'connected' || isRunning || (tool.id === 'mesh.retopology' && !validQuadTarget)}
                    onClick={() => void runMeshOperation(tool.id)}
                  >
                    Run {tool.title}
                  </Button>
                )}
                </div>
                </AccordionContent>
              </AccordionItem>
            );
          })}
        </Accordion>
      </div>

      <Disclosure className="mx-4 mb-4 border-y py-3 text-xs/relaxed" title="Retopology availability"
        contentClassName="[&>p]:my-2 [&>p]:text-muted-foreground">
        <p>General high-to-low texture baking remains unavailable. Experimental QuadriFlow retopology is unavailable until a signature-verified, manifest-matched managed sidecar is supported; adjacent binaries are intentionally ignored. Editable quads are preserved in a separate geometry-only OBJ, while supported base-color/metallic-roughness maps are projected only into the triangulated GLB preview. Mesh tools create derived assets and keep originals unchanged. The repository’s project license has not been selected; upstream model terms remain separate.</p>
        <div className="my-2 flex flex-wrap justify-between gap-2 border-t pt-2"><span>QuadriFlow backend</span><span className="text-muted-foreground">{quadriflowBackend?.status.replaceAll('_', ' ') ?? 'Connect engine'}</span></div>
        {quadriflowBackend && <><p>{quadriflowBackend.message}</p><p>{quadriflowBackend.next_step}</p></>}
        <Button onClick={() => void installQuadriflowBackend()} disabled={!quadriflowBackend?.install_available || installingBackend}>{installingBackend ? 'Installing…' : quadriflowBackend?.install_available ? 'Install QuadriFlow' : 'Install unavailable'}</Button>
        <div className="my-2 flex flex-wrap justify-between gap-2 border-t pt-2"><span>CUDA backend</span><span className="text-muted-foreground">{backendStatus('pixal3d-cuda')}</span></div>
        <div className="my-2 flex flex-wrap justify-between gap-2 border-t pt-2"><span>Apple MLX backend</span><span className="text-muted-foreground">{backendStatus('pixal3d-mlx')}</span></div>
      </Disclosure>
      </>}
    </>;
  }

  const toolRail = <div role="group" aria-label="Model tools" data-slot="model-tools" className={sidebar.narrow ? '' : 'h-full'}>
    <SidebarMenu className={sidebar.narrow ? 'grid grid-cols-4 p-1.5' : 'p-1.5'}>
      {([
        { key: 'generation', label: 'New model', icon: ImageSquareIcon },
        { key: 'optimize', label: 'Optimize', icon: SlidersHorizontalIcon },
        { key: 'tools', label: 'More tools', icon: WrenchIcon },
        { key: 'jobs', label: 'Jobs', icon: ListChecksIcon },
      ] as const).map(({ key, label, icon: Icon }) => (
        <SidebarMenuItem key={key}>
          <SidebarMenuButton type="button" isActive={activeTool === key}
            className="h-auto min-w-0 flex-col gap-0.5 px-1 py-1 active:bg-transparent hover:bg-transparent data-open:hover:bg-transparent data-active:bg-transparent [&_svg]:size-5"
            aria-current={activeTool === key ? true : undefined} aria-expanded={openInspector === key}
            aria-controls={openInspector === key ? 'workbench-inspector' : undefined}
            aria-haspopup={inspectorIsSheet ? 'dialog' : undefined}
            disabled={(key === 'optimize' || key === 'tools') && selectedAsset?.kind !== 'mesh'}
            onClick={event => selectInspector(key, event.currentTarget)}>
            <span data-slot="tool-icon" aria-hidden="true"
              className="grid size-8 shrink-0 place-items-center rounded-md transition-colors group-hover/menu-button:bg-sidebar-accent group-aria-[current=true]/menu-button:bg-sidebar-accent group-aria-[current=true]/menu-button:text-primary motion-reduce:transition-none">
              <Icon className="size-5" />
            </span>
            <span data-slot="tool-label" className="text-[0.6875rem] leading-tight text-muted-foreground group-hover/menu-button:text-foreground group-aria-[current=true]/menu-button:text-foreground">{label}</span>
          </SidebarMenuButton>
        </SidebarMenuItem>
      ))}
    </SidebarMenu>
  </div>;
  const desktopInspector = openInspector && !inspectorIsSheet ? (
    <aside className="h-full min-w-0 overflow-auto overscroll-contain" id="workbench-inspector" data-tool={openInspector}
      aria-labelledby="workbench-inspector-title" onKeyDown={event => {
        if (event.key === 'Escape') { event.stopPropagation(); closeInspector(); }
      }}>
      <SidebarHeader className="flex-row items-center justify-between border-b px-4 py-2 font-heading text-sm font-medium">
        <h2 id="workbench-inspector-title">{inspectorTitle}</h2>
        <Button variant="ghost" size="icon-sm" title={closeInspectorLabel} aria-label={closeInspectorLabel} onClick={closeInspector}><SidebarSimpleIcon aria-hidden="true" /></Button>
      </SidebarHeader>
      {inspectorBody()}
    </aside>
  ) : null;

  return (
    <>
      <Toaster theme={theme} closeButton visibleToasts={3} containerAriaLabel="Notifications"
        offset={{ right: 12, bottom: sidebar.narrow ? 80 : 12 }}
        mobileOffset={{ left: 12, right: 12, bottom: 80 }} />
      <SettingsPage api={api} section={settingsSection} backendId={settingsBackend} runtime={runtime}
        connection={connection} connectionError={connectionError} busy={busy || isRunning || installingBackend}
        jobFeedback={jobFeedback} jobs={recentJobs} operationLabel={studioOperationLabel}
        theme={theme} onThemeChange={setTheme}
        onVisit={openSettings} onProfileChange={setSettingsBackend} onClose={closeSettings}
        onReconnect={() => void connect(true)} onRefreshJobs={refreshRecentJobs} onRuntimeChange={setRuntime}
        onRuntimeAction={(backend, action) => void runRuntimeAction(backend, action)} />
      <div className={`${settingsSection ? 'hidden' : 'flex'} min-h-dvh min-w-0 flex-col bg-background min-[881px]:h-dvh min-[881px]:overflow-hidden`} onDragOver={(event) => {
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
      <header className="z-10 flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-b bg-background px-4 py-2">
        <span className="font-heading text-sm font-medium">meshharbor</span>
          {selectedAsset?.kind === 'mesh' && <div className="order-last flex w-full flex-wrap items-center gap-2 md:order-0 md:ml-auto md:w-auto" role="group" aria-label="File actions">
            {quadArtifact && <Button variant="secondary" size="sm" title="Editable quad topology without textures or UVs" disabled={downloading || comparisonLoadingSide !== null || connection !== 'connected'} onClick={() => void downloadResult(quadArtifact)}>Quad OBJ</Button>}
            {selectedAsset.kind === 'mesh' && <Button variant={exportIsPrimary ? 'default' : 'secondary'} size="sm" data-review-action={exportIsPrimary || undefined} title="Export GLB" aria-label="Export selected GLB" disabled={downloading || comparisonLoadingSide !== null || connection !== 'connected'} onClick={() => void downloadResult()}><DownloadSimpleIcon aria-hidden="true" /> <span>Export GLB</span></Button>}
          </div>}
        <div className={selectedAsset?.kind === 'mesh' ? 'ml-auto flex items-center gap-2 md:ml-0' : 'ml-auto flex items-center gap-2'}>
          <Button variant={sidebar.open ? 'secondary' : 'ghost'} size="sm" ref={sidebar.triggerRef}
            aria-expanded={sidebar.open} aria-haspopup={sidebar.narrow ? 'dialog' : undefined}
            aria-controls={sidebar.open ? (sidebar.narrow ? 'workbench-sidebar-drawer' : 'workbench-sidebar-dock') : undefined}
            onClick={sidebar.toggleInventory}><SquaresFourIcon aria-hidden="true" /> Inventory</Button>
          <Button variant="ghost" size="sm" ref={settingsTrigger} onClick={() => openSettings('models')}><GearSixIcon data-icon="inline-start" aria-hidden="true" /> Settings</Button>
        </div>
      </header>

      <main className="relative flex min-h-0 min-w-0 flex-1 flex-col" ref={workbenchRef} data-connection={connection}>
          <h1 className="sr-only">MeshHarbor Studio</h1>
          <input ref={fileInputRef} className="sr-only" aria-label="Choose GLB files" tabIndex={-1} type="file" multiple accept=".glb,model/gltf-binary" onChange={(event) => { if (event.currentTarget.files) void importFiles(event.currentTarget.files); event.currentTarget.value = ''; }} />
          <input ref={inventoryInputRef} className="sr-only" aria-label="Choose image files" tabIndex={-1} type="file" multiple accept=".png,.jpg,.jpeg,image/png,image/jpeg" onChange={(event) => { if (event.currentTarget.files) void importFiles(event.currentTarget.files); event.currentTarget.value = ''; }} />
          {!showInspectorSheet && !settingsSection && workbenchActivity && <div className={`pointer-events-none fixed inset-x-3 z-20 mx-auto grid max-w-sm gap-2 [&>*]:pointer-events-auto ${sidebar.narrow ? 'bottom-20' : 'bottom-3'}`}>
            {workbenchActivity}
          </div>}

            <WorkbenchLayout state={settingsSection && sidebar.narrow ? { ...sidebar, open: false } : sidebar} inspector={desktopInspector} tools={toolRail}
              assets={
              <Tabs value={assetFilters.kind} onValueChange={kind => {
                if (kind === 'mesh' || kind === 'image') applyAssetBrowserFilters({ ...EMPTY_ASSET_BROWSER_FILTERS, kind });
              }} className="@container min-w-0 text-xs" id="asset-library">
                <TabsList aria-label="Asset types" className="mx-3 mt-2 grid w-auto grid-cols-2">
                  <TabsTrigger value="mesh">3D models</TabsTrigger>
                  <TabsTrigger value="image">Images</TabsTrigger>
                </TabsList>
                <TabsContent value={assetFilters.kind} className="focus-visible:outline-ring">
                <div className="grid min-w-0 gap-2 border-b px-3 py-3">
                  <div className="flex min-w-0 items-center gap-2">
                    <Button variant="outline" size="sm" className="min-w-0 flex-1 justify-center" disabled={busy || connection !== 'connected'}
                      onClick={() => (assetFilters.kind === 'mesh' ? fileInputRef : inventoryInputRef).current?.click()}>
                      {assetFilters.kind === 'mesh' ? <><CubeIcon aria-hidden="true" /> Open GLB</> : <><PlusIcon aria-hidden="true" /> Import images</>}
                    </Button>
                    <Button variant="ghost" size="icon-sm" aria-label="Refresh assets" title="Refresh assets"
                      onClick={() => void refreshAssets()} disabled={connection !== 'connected'}><ArrowsClockwiseIcon aria-hidden="true" /></Button>
                  </div>
                {showLibraryFilters && <><form role="search" aria-label="Asset inventory" className="relative" onSubmit={(event) => {
                  event.preventDefault();
                  applyAssetBrowserFilters({ ...assetFilters, query: assetSearchInput.trim() });
                }}>
                    <Input
                      id="asset-filename"
                      className="pr-8"
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
                  <Button type="submit" aria-label="Search" title="Search" variant="ghost" size="icon-xs"
                    className="absolute top-1/2 right-1 -translate-y-1/2" disabled={connection !== 'connected'}><MagnifyingGlassIcon aria-hidden="true" /></Button>
                </form>
                <Disclosure className="min-w-0 text-muted-foreground" title={hasActiveAssetFilters ? 'Filters · Active' : 'Filters'}>
                  <div className="mt-2 grid gap-2">
                  <Field className="min-w-0">
                    <FieldLabel htmlFor="asset-origin">Origin</FieldLabel>
                    <NativeSelect
                      className="w-full"
                      id="asset-origin"
                      aria-label="Filter assets by origin"
                      value={assetFilters.origin}
                      disabled={connection !== 'connected'}
                      onChange={(event) => applyAssetBrowserFilters({
                        ...assetFilters,
                        query: assetSearchInput.trim(),
                        origin: event.currentTarget.value as AssetOriginFilter,
                      })}
                    >
                      <NativeSelectOption value="all">All assets</NativeSelectOption>
                      <NativeSelectOption value="original">Originals</NativeSelectOption>
                      <NativeSelectOption value="derived">Generated results</NativeSelectOption>
                    </NativeSelect>
                  </Field>
                  {hasActiveAssetFilters && (
                    <Button type="button" variant="ghost" size="sm" className="justify-self-start" onClick={() => applyAssetBrowserFilters({ ...EMPTY_ASSET_BROWSER_FILTERS, kind: assetFilters.kind })}>
                      Clear filters
                    </Button>
                  )}
                  </div>
                </Disclosure></>}
                </div>
                <div className="grid min-w-0 gap-3 px-3 py-3">
                {matchingAssets.length > 0 && <p className="text-xs text-muted-foreground" aria-live="polite">
                  {assetPageLoading ? 'Loading assets…' : `${matchingAssets.length} loaded ${hasActiveAssetFilters ? 'result' : 'asset'}${matchingAssets.length === 1 ? '' : 's'}`}
                </p>}
                {matchingAssets.length === 0
                  ? <Empty role="status"><EmptyHeader><EmptyDescription>
                      {assetPageLoading ? 'Loading assets…' : hasActiveAssetFilters
                        ? 'No matching assets' : assetFilters.kind === 'mesh' ? 'Opened GLBs and 3D results appear here' : 'Imported images appear here'}
                    </EmptyDescription></EmptyHeader></Empty>
                  : <div className={ASSET_GALLERY_GRID}>{matchingAssets.map(renderAssetCard)}</div>}
                {pinnedAssets.length > 0 && <section className="grid grid-cols-1 gap-2 border-t pt-3" aria-label="Current selection and comparison">
                  <h3 className="text-xs text-muted-foreground">Current selection</h3>
                  <div className={ASSET_GALLERY_GRID}>{pinnedAssets.map(renderAssetCard)}</div>
                </section>}
                {assetNextCursor && (
                  <div className="flex justify-center">
                    <Button type="button" variant="outline" size="sm" onClick={() => void refreshAssets(true)} disabled={assetPageLoading || connection !== 'connected'}>
                      {assetPageLoading ? 'Loading…' : 'Load more assets'}
                    </Button>
                  </div>
                )}
                </div>
                </TabsContent>
              </Tabs>
              }
            >

            <section className="flex min-h-0 min-w-0 flex-1 flex-col bg-muted dark:bg-neutral-800" aria-label={selectedAsset ? 'Selected asset' : 'Start a model'}>
              {!selectedAsset ? <div className="flex min-h-0 flex-1 overflow-y-auto">
                <div className="m-auto grid w-full max-w-4xl items-center justify-center gap-4 p-6 lg:grid-cols-[minmax(0,22rem)_minmax(0,24rem)] lg:gap-8">
                <div className="order-2 grid min-w-0 gap-4 lg:order-first">
                  <h2 className="font-heading text-2xl font-medium lg:text-3xl">Start with a model or an image</h2>
                  <p className="text-sm text-muted-foreground">Inspect and refine a GLB, or turn a reference image into a 3D model on this computer.</p>
                  <div className="flex flex-wrap gap-2">
                    <Button onClick={() => {
                      if (assetFilters.kind !== 'mesh') applyAssetBrowserFilters(EMPTY_ASSET_BROWSER_FILTERS);
                      sidebar.setOpen(true);
                    }}><CubeIcon data-icon="inline-start" aria-hidden="true" /> Browse inventory</Button>
                    <Button variant="outline" onClick={event => selectInspector('generation', event.currentTarget)}><ImageSquareIcon data-icon="inline-start" aria-hidden="true" /> Create from image</Button>
                  </div>
                  <p className="text-xs text-muted-foreground">Drop a GLB, PNG or JPEG here</p>
                </div>
                <MeshStudy />
                </div>
              </div> : <div className="flex min-h-0 min-w-0 flex-1 flex-col">
                <div className="flex shrink-0 flex-wrap items-center gap-2 border-b bg-card p-2 text-card-foreground">
                <div className="flex min-w-0 flex-1 items-center justify-between gap-2 max-sm:basis-full">
                  <div className="flex min-w-0 items-center gap-2 text-xs">{selectedAsset.kind === 'image' ? <FileImageIcon aria-hidden="true" /> : <CubeIcon aria-hidden="true" />} <span className="truncate font-medium" title={selectedAsset.name}>{selectedAsset.name}</span></div>
                  {selectedComparisonPair && (
                    <ToggleGroup className="shrink-0" type="single" orientation="horizontal" size="sm" variant="outline" spacing={0}
                      aria-label="Preview version" aria-busy={comparisonLoadingSide !== null}
                      disabled={comparisonLoadingSide !== null || busy || connection !== 'connected'}
                      value={activeView} onValueChange={value => {
                        if ((value === 'source' || value === 'result') && value !== activeView) void selectComparisonSide(value);
                      }}>
                      <ToggleGroupItem value="source">Original</ToggleGroupItem>
                      <ToggleGroupItem value="result">Result</ToggleGroupItem>
                    </ToggleGroup>
                  )}
                </div>
                {selectedAsset.kind === 'image' && <Button variant="secondary" size="sm" disabled={busy || isRunning} onClick={event => {
                  inspectorTrigger.current = event.currentTarget;
                  setGenerationSource(selectedAsset);
                  setPreferredViewMode('single');
                  setSelectedTool('generation');
                  setInspector('generation');
                }}><ImageSquareIcon data-icon="inline-start" aria-hidden="true" /> Use as reference</Button>}
                </div>
                <ModelPreview asset={previewAsset} comparison={previewComparison} loadModel={loadPreview} onError={reportPreviewError} theme={theme} />
                {showVersionStats && <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-t bg-card px-3 py-2 text-xs tabular-nums text-muted-foreground [&_strong]:font-medium [&_strong]:text-foreground" role="group" aria-label="Original to result comparison">
                  {typeof beforeTriangles === 'number' && typeof afterTriangles === 'number' && <span>Triangles <strong>{beforeTriangles.toLocaleString()} → {afterTriangles.toLocaleString()}</strong></span>}
                  {comparisonSizes && <span>GLB size <strong>{comparisonSizes}</strong></span>}
                </div>}
                {selectedAsset && (
                  <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-t bg-card px-3 py-2 text-xs tabular-nums">
                    <div className="flex items-center gap-2 text-muted-foreground"><Badge variant="outline">{selectedAsset.kind === 'mesh' ? 'GLB' : selectedAsset.mime_type === 'image/png' ? 'PNG' : 'JPG'}</Badge><div><span>{formatSize(selectedAsset.byte_size)} · {selectedAsset.kind === 'mesh' ? `${Number(selectedAsset.metadata.mesh_count ?? 0)} mesh${Number(selectedAsset.metadata.mesh_count ?? 0) === 1 ? '' : 'es'}` : 'Reference image'}</span></div></div>
                    <Separator orientation="vertical" className="ml-auto hidden h-4 sm:block" />
                    {selectedAsset.kind === 'mesh' ? <><div className="grid gap-1 [&>span]:text-muted-foreground [&>strong]:font-medium"><span>Triangles</span><strong>{triangleCount.toLocaleString()}</strong></div><div className="grid gap-1 [&>span]:text-muted-foreground [&>strong]:font-medium"><span>Materials</span><strong>{Number(selectedAsset.metadata.material_count ?? 0)}</strong></div></> : <div className="grid gap-1 [&>span]:text-muted-foreground [&>strong]:font-medium"><span>Input</span><strong>{selectedAsset.mime_type === 'image/png' ? 'PNG' : 'JPEG'}</strong></div>}
                    <div className="grid gap-1 [&>span]:text-muted-foreground [&>strong]:font-medium"><span>Version</span><strong>{selectedAsset.source_asset_id ? 'Result' : 'Original'}</strong></div>
                  </div>
                )}
              {reportCard && (
                <Disclosure className="max-h-60 shrink-0 overflow-auto border-t bg-card p-3 text-xs" label={`${reportCard.title} result`} title="Operation report">
                  <div className="mt-3 font-medium"><strong>{reportCard.title} complete</strong></div>
                  {!reportCard.reportAvailable && <p className="mt-2 text-muted-foreground">The operation completed, but its report payload is missing or unreadable.</p>}
                  <div className="mt-3 grid grid-cols-2 gap-2 sm:grid-cols-3">
                    {reportCard.metrics.map((metric) => (
                      <div className="grid min-w-0 gap-1 rounded-md border p-2 break-words [&>span]:text-muted-foreground" key={metric.label}>
                        <span>{metric.label}</span>
                        <strong>{metric.value}</strong>
                      </div>
                    ))}
                  </div>
                  {reportCard.lists.map((list) => (
                    <div className="mt-3 grid gap-1 border-t pt-2 text-muted-foreground [&>ul]:ml-4 [&>ul]:list-disc" key={list.label}>
                      <strong>{list.label}</strong>
                      {list.state === 'unavailable' && <p>Not reported.</p>}
                      {list.state === 'empty' && <p>None reported.</p>}
                      {list.state === 'partial' && <p>Some entries were invalid or omitted.</p>}
                      {list.values.length > 0 && <ul>{list.values.map((value, index) => <li key={`${index}-${value}`}>{value}</li>)}</ul>}
                      {list.omittedCount > 0 && <p>{list.omittedCount.toLocaleString()} additional entries are not shown.</p>}
                    </div>
                  ))}
                  <section className="mt-3 grid gap-1 border-t pt-2 text-muted-foreground [&>ul]:ml-4 [&>ul]:list-disc" aria-label="Warnings">
                    <strong>Warnings</strong>
                    {reportCard.warningsState === 'unavailable' && <p>{reportCard.warningsMessage}</p>}
                    {reportCard.warningsState === 'empty' && <p>No warnings reported.</p>}
                    {reportCard.warningFormatIssue && <p>Some warning data was missing or invalid; all readable warning text is listed below.</p>}
                    {reportCard.warnings.length > 0 && <ul>{reportCard.warnings.map((warning, index) => <li key={`${index}-${warning}`}>{warning}</li>)}</ul>}
                  </section>
                  <Disclosure className="mt-3" title="Asset and job IDs">
                    <dl className="mt-2 grid gap-1 break-all text-muted-foreground [&>div]:grid [&>div]:grid-cols-[3rem_minmax(0,1fr)]">
                      <div><dt>Job</dt><dd><code>{reportCard.jobId}</code></dd></div>
                      <div><dt>Input</dt><dd><code>{reportCard.inputAssetId ?? 'Not reported'}</code></dd></div>
                      <div><dt>Result</dt><dd><code>{reportCard.resultAssetId ?? 'Not reported'}</code></dd></div>
                    </dl>
                  </Disclosure>
                </Disclosure>
              )}
              </div>}

            </section>

            </WorkbenchLayout>
          {showInspectorSheet && !settingsSection && <Sheet open={showInspectorSheet} onOpenChange={open => { if (!open) setInspector(null); }}>
            <SheetContent side="left" id="workbench-inspector" data-tool={openInspector} aria-describedby={undefined}
              className="overflow-auto overscroll-contain max-sm:w-full" onCloseAutoFocus={event => {
              event.preventDefault();
              restoreInspectorFocus();
            }}>
              <SheetHeader><SheetTitle>{inspectorTitle}</SheetTitle></SheetHeader>
              {workbenchActivity && <div className="grid gap-2 px-4">{workbenchActivity}</div>}
              {inspectorBody()}
            </SheetContent>
          </Sheet>}
      </main>
      {dragging && <div className="pointer-events-none fixed inset-0 z-30 grid place-items-center bg-background/80 p-4"><div className="grid gap-2 rounded-lg border border-dashed bg-card p-6 text-center text-sm"><span className="text-lg text-primary">↓</span><strong>Drop a GLB or source image here</strong><span>Import starts locally on your computer</span></div></div>}

      </div>
    </>
  );
}

function formatJobTime(timestamp: string): string {
  const date = new Date(Number(timestamp));
  return Number.isNaN(date.getTime()) ? 'time unavailable' : date.toLocaleString();
}

function displaysJobProgress(operation: string): boolean {
  return operation !== 'image.generate' && !operation.startsWith('runtime.');
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
    case 'runtime.pixal3d.cuda_install_multiview': return 'Install multiview weights';
    case 'runtime.pixal3d.convert_naf': return 'Convert NAF checkpoint';
    default: return meshOperationOptions.find(tool => tool.id === operation)?.title ?? operation;
  }
}
