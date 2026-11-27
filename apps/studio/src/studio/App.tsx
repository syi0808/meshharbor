import { ArrowDownIcon, ArrowsClockwiseIcon, ArrowsInIcon, BoundingBoxIcon, BroomIcon, CubeIcon, CubeTransparentIcon, DownloadSimpleIcon, FileImageIcon, GaugeIcon, GearSixIcon, GridFourIcon, ImageSquareIcon, ListChecksIcon, MagnifyingGlassIcon, PlusIcon, SlidersHorizontalIcon, SquaresFourIcon, type Icon } from '@phosphor-icons/react';
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
  generationLabel,
  isActiveJob,
  isGenerationOperation,
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
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty';
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from '@/components/ui/input-group';
import { Item, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Separator } from '@/components/ui/separator';
import { SidebarFooter, SidebarHeader, SidebarMenu, SidebarMenuButton, SidebarMenuItem } from '@/components/ui/sidebar';
import { Slider } from '@/components/ui/slider';
import { Spinner } from '@/components/ui/spinner';
import { Toaster } from '@/components/ui/sonner';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Textarea } from '@/components/ui/textarea';
import { ModelPreview } from './ModelPreview';
import { BlenderExport } from './BlenderExport';
import { ModelVersionHistory } from './ModelVersionHistory';
import { AssetGalleryCard } from './AssetGalleryCard';
import { Disclosure } from './Disclosure';
import { JobsPanel, PendingGenerationCards, type StudioActivity } from './JobsPanel';
import { JOB_PAGE_SIZE, generationJobName, jobStage, mergeJobRecords } from './job-activity';
import { MeshStudy } from './MeshStudy';
import { GenerationParameterFields, generationParameters, selectableGenerationProfiles } from './GenerationSettings';
import { MeshToMeshSettings, selectableMeshToMeshModels } from './MeshToMeshSettings';
import { buildPosedInput, defaultViews, newView, PosedReferences, SingleReference, type ViewDraft } from './GenerationReferences';
import { HelpTip } from './HelpTip';
import { JobDetails } from './JobDetails';
import { NotificationHistory } from './NotificationHistory';
import { notify } from './notify';
import { SettingsPage, settingsSectionFromPath, type RuntimeAction, type SettingsSection } from './SettingsPage';
import { applyStudioTheme, loadStudioTheme } from './theme';
import { useWorkbenchSidebar, WorkbenchLayout } from './WorkbenchSidebar';
import { useRetopologyPreflight } from './useRetopologyPreflight';
import { formatVersionSizes, shouldHighlightExport, type Inspector } from './workbench-state';
import type { Artifact, Asset, AssetKind, GenerationInput, GenerationInputMode, GenerationViewMode, Job, MeshOperationId, Operation, PosedViewMode, RuntimeBackendId, RuntimeInfo } from './types';

type Connection = 'disconnected' | 'connecting' | 'connected';
type AssetOriginFilter = 'all' | 'original' | 'derived';
type AssetImportPurpose = 'library' | 'reference' | { mode: PosedViewMode; maxViews: number; index?: number };
const viewLabels: Record<GenerationViewMode, string> = { single: '1 view', four: '4 views', six: '6 views', custom: 'Custom views' };
const inputModeLabels: Record<GenerationInputMode, string> = { image: 'Image', text: 'Text' };
const MAX_PROMPT_BYTES = 4096;
interface AssetBrowserFilters {
  query: string;
  kind: AssetKind;
  origin: AssetOriginFilter;
  allVersions?: boolean;
}

const ASSET_PAGE_SIZE = 50;
const DEFAULT_VOXEL_RESOLUTION = 96;
const MIN_VOXEL_RESOLUTION = 16;
const MAX_VOXEL_RESOLUTION = 160;
const meshParameterFields = {
  'uv.unwrap': [
    { name: 'resolution', label: 'Atlas resolution', min: 64, max: 4096, step: '1', initial: '1024' },
    { name: 'padding', label: 'Chart padding', min: 0, max: 64, step: '1', initial: '8' },
  ],
  'render.optimize': [
    { name: 'overdraw_threshold', label: 'Overdraw threshold', min: 1, max: 1.5, step: 'any', initial: '1.05' },
  ],
} as const;
const ASSET_GALLERY_GRID = 'grid min-w-0 grid-cols-2 gap-2 @min-[380px]:grid-cols-3';
const EMPTY_ASSET_BROWSER_FILTERS: AssetBrowserFilters = { query: '', kind: 'mesh', origin: 'all' };

function jobsFromLocation() {
  return window.location.pathname === '/jobs'
    ? { jobId: new URLSearchParams(window.location.search).get('job') }
    : null;
}

function toAssetPageFilters(filters: AssetBrowserFilters): AssetPageFilters {
  const query = filters.query.trim();
  return {
    ...(query ? { query } : {}),
    kind: filters.kind,
    ...(filters.kind === 'mesh' && !filters.allVersions ? { modelsOnly: true } : {}),
    ...(filters.origin === 'all' ? {} : { derived: filters.origin === 'derived' }),
  };
}

function matchesAssetBrowserFilters(asset: Asset, filters: AssetBrowserFilters): boolean {
  const displayName = filters.allVersions ? asset.name : asset.model?.name ?? asset.name;
  const derived = filters.allVersions ? Boolean(asset.source_asset_id || asset.metadata.origin === 'generated')
    : asset.model?.generated ?? Boolean(asset.source_asset_id);
  return (!filters.query || displayName.toLowerCase().includes(filters.query.toLowerCase()))
    && asset.kind === filters.kind
    && (filters.origin === 'all' || derived === (filters.origin === 'derived'));
}

function inventoryAssetKey(asset: Asset, filters: AssetBrowserFilters): string {
  return filters.allVersions ? asset.id : asset.model?.id ?? asset.id;
}

function mergeCachedAssets(incoming: Asset[], cached: Asset[]): Asset[] {
  const ids = new Set(incoming.map(asset => asset.id));
  const models = new Map(incoming.flatMap(asset => asset.model ? [[asset.model.id, asset.model] as const] : []));
  return [...incoming, ...cached.filter(asset => !ids.has(asset.id)).map(asset => {
    const model = asset.model && models.get(asset.model.id);
    return model && model.current_asset_id !== asset.model?.current_asset_id ? { ...asset, model } : asset;
  })];
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

const meshOperationOptions: Array<{ id: MeshOperationId; title: string; label?: string; icon: Icon; description: string }> = [
  {
    id: 'mesh.retopology_lato2',
    title: 'Mesh to Mesh',
    icon: SquaresFourIcon,
    description: 'Rebuild a closed triangle mesh with LATO.2 and rebake the original colors. Requires a static GLB with one indexed primitive and an embedded PNG base-color map.',
  },
  {
    id: 'mesh.retopology',
    title: 'Quad retopology (experimental)',
    label: 'Retopology',
    icon: GridFourIcon,
    description: 'Build a quad mesh with QuadriFlow. Supported PNG base color and metallic/roughness maps appear only in the GLB preview; the quad OBJ is geometry-only.',
  },
  {
    id: 'mesh.cleanup',
    title: 'Cleanup',
    icon: BroomIcon,
    description: 'Remove invalid and duplicate faces, then reduce redundant triangles within a small shape and UV error tolerance. No triangle target is needed.',
  },
  {
    id: 'mesh.voxel_remesh',
    title: 'Voxel Remesh',
    icon: CubeTransparentIcon,
    description: 'Rebuild one indexed triangle mesh as a closed surface. Fine details can change. The new version contains geometry only; UVs, materials, and textures stay on the source.',
  },
  {
    id: 'uv.unwrap',
    title: 'UV atlas rebake',
    icon: BoundingBoxIcon,
    description: 'Lay out a new UV atlas and copy the existing embedded PNG base color and metallic/roughness into it. This works on one supported primitive and does not transfer detail from a different mesh.',
  },
  {
    id: 'render.optimize',
    title: 'Render optimization',
    icon: GaugeIcon,
    description: 'Reorder triangles so the GPU can reuse vertices and avoid shading hidden pixels. The shape and triangle count stay the same; blended triangles keep their order.',
  },
  {
    id: 'mesh.compress',
    title: 'Compression',
    icon: ArrowsInIcon,
    description: 'Shrink geometry with KHR_meshopt_compression. Other viewers need a compatible decoder.',
  },
];

export function App() {
  const [theme, setTheme] = useState(loadStudioTheme);
  useLayoutEffect(() => { applyStudioTheme(theme); }, [theme]);
  const [connection, setConnection] = useState<Connection>('disconnected');
  const [settingsSection, setSettingsSection] = useState(() => settingsSectionFromPath(window.location.pathname));
  const [focusedJobId, setFocusedJobId] = useState(() => jobsFromLocation()?.jobId ?? null);
  const [settingsBackend, setSettingsBackend] = useState<string | null>(null);
  const settingsTrigger = useRef<HTMLButtonElement>(null);
  const sidebar = useWorkbenchSidebar(jobsFromLocation() ? 'jobs' : 'inventory');
  const { showPanel } = sidebar;
  useEffect(() => {
    function onPopState() {
      setSettingsSection(settingsSectionFromPath(window.location.pathname));
      const jobs = jobsFromLocation();
      if (jobs) showPanel('jobs');
      setFocusedJobId(jobs?.jobId ?? null);
    }
    window.addEventListener('popstate', onPopState);
    return () => window.removeEventListener('popstate', onPopState);
  }, [showPanel]);
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [operations, setOperations] = useState<Operation[]>([]);
  const [assets, setAssets] = useState<Asset[]>([]);
  const [inventoryAssets, setInventoryAssets] = useState<Asset[]>([]);
  const [assetSearchInput, setAssetSearchInput] = useState('');
  const [storedItemId, setStoredItemId] = useState('');
  const assetSearchInputRef = useRef('');
  const [assetFilters, setAssetFilters] = useState<AssetBrowserFilters>(EMPTY_ASSET_BROWSER_FILTERS);
  const assetFiltersRef = useRef(assetFilters);
  const assetBrowserInteractionRevision = useRef(0);
  const assetSelectionInteractionRevision = useRef(0);
  const [assetNextCursor, setAssetNextCursor] = useState<string | null>(null);
  const [assetPageLoading, setAssetPageLoading] = useState(false);
  const assetPageRequestGuard = useRef(new LatestRequestGuard());
  const [jobSnapshot, setJobSnapshot] = useState<Job[]>([]);
  const [detailsTarget, setDetailsTarget] = useState<Job | null>(null);
  const detailsTrigger = useRef<HTMLButtonElement | null>(null);
  const observedJobStates = useRef(new Map<string, Job['state']>());
  const localGenerations = useRef(new Set<string>());
  const [jobFeedLoaded, setJobFeedLoaded] = useState(false);
  const jobFeedInFlight = useRef<Promise<boolean> | null>(null);
  const observedJobResults = useRef(new Set<string>());
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selectedIdRef = useRef<string | null>(null);
  const [pendingSelection, setPendingSelection] = useState<Job | null>(null);
  const pendingSelectionRef = useRef<Job | null>(null);
  const [comparisonPair, setComparisonPair] = useState<AssetComparisonPair | null>(null);
  const comparisonPairRef = useRef(comparisonPair);
  const [comparisonLoadingSide, setComparisonLoadingSide] = useState<AssetComparisonSide | null>(null);
  const [targetTriangles, setTargetTriangles] = useState(4000);
  const [quadTargetEnabled, setQuadTargetEnabled] = useState(false);
  const [targetQuads, setTargetQuads] = useState('10000');
  const [voxelResolution, setVoxelResolution] = useState(String(DEFAULT_VOXEL_RESOLUTION));
  const requestedQuads = quadTargetEnabled ? Number(targetQuads) : undefined;
  const validQuadTarget = requestedQuads === undefined
    || (Number.isInteger(requestedQuads) && requestedQuads >= 4 && requestedQuads <= 50000);
  const requestedVoxelResolution = Number(voxelResolution);
  const validVoxelResolution = Number.isInteger(requestedVoxelResolution)
    && requestedVoxelResolution >= MIN_VOXEL_RESOLUTION && requestedVoxelResolution <= MAX_VOXEL_RESOLUTION;
  const [maxErrorDraft, setMaxErrorDraft] = useState('0.02');
  const maxError = Number(maxErrorDraft);
  const validMaxError = maxErrorDraft !== '' && Number.isFinite(maxError) && maxError >= 0 && maxError <= 1;
  const [meshParameterDrafts, setMeshParameterDrafts] = useState<Record<string, string>>({});
  const [generationModelId, setGenerationModelId] = useState('');
  const [generationDrafts, setGenerationDrafts] = useState<Record<string, Record<string, string>>>({});
  const [generationSource, setGenerationSource] = useState<Asset | null>(null);
  const [generationPrompt, setGenerationPrompt] = useState('');
  const [preferredInputMode, setPreferredInputMode] = useState<GenerationInputMode>('image');
  const [preferredViewMode, setPreferredViewMode] = useState<GenerationViewMode>('single');
  const [posedDrafts, setPosedDrafts] = useState<Partial<Record<PosedViewMode, ViewDraft[]>>>({});
  const [generationResult, setGenerationResult] = useState<Job | null>(null);
  const historySelectionGuard = useRef(new LatestRequestGuard());
  const recentJobsRequestGuard = useRef(new LatestRequestGuard());
  const [job, setJobState] = useState<Job | null>(null);
  const [stopTarget, setStopTarget] = useState<Job | null>(null);
  const [stoppingJob, setStoppingJob] = useState(false);
  const stopTrigger = useRef<HTMLButtonElement | null>(null);
  const [retryTarget, setRetryTarget] = useState<Job | null>(null);
  const retryTrigger = useRef<HTMLButtonElement | null>(null);
  const jobRef = useRef<Job | null>(null);
  const setJob = useCallback((next: Job | null) => {
    if (jobRef.current?.id !== next?.id) historySelectionGuard.current.invalidate();
    jobRef.current = next;
    if (next) setJobSnapshot(current => mergeJobRecords([next], current));
    setJobState(next);
  }, []);
  const [reportJob, setReportJob] = useState<Job | null>(null);
  const [resolvedPreviewSource, setResolvedPreviewSource] = useState<Asset | null>(null);
  const [busy, setBusy] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const downloadInFlight = useRef(false);
  const [inspector, setInspector] = useState<Inspector>('generation');
  const draggedImage = useRef<Asset | null>(null);
  const [dragging, setDragging] = useState(false);
  const [importProgress, setImportProgress] = useState<AssetImportProgress | null>(null);
  const [settingsActivity, setSettingsActivity] = useState<StudioActivity | null>(null);
  const importStartedAt = useRef(0);
  const backendInstallStartedAt = useRef(0);
  const exportStartedAt = useRef(0);
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
  const matchingAssets = inventoryAssets.filter(asset => matchesAssetBrowserFilters(asset, assetFilters));
  const shownAssetKeys = new Set(matchingAssets.map(asset => inventoryAssetKey(asset, assetFilters)));
  const pinnedKeys = new Set(assets.filter(asset => pinnedAssetIds.has(asset.id)).map(asset => inventoryAssetKey(asset, assetFilters)));
  const pinnedAssets = assets.filter(asset => {
    const key = inventoryAssetKey(asset, assetFilters);
    if (asset.kind !== assetFilters.kind || (!assetFilters.allVersions && asset.model && asset.id !== asset.model.current_asset_id)
      || !pinnedKeys.has(key) || shownAssetKeys.has(key)) return false;
    shownAssetKeys.add(key);
    return true;
  });
  const hasActiveAssetFilters = assetFilters.query !== ''
    || assetFilters.origin !== 'all' || assetFilters.allVersions === true;
  const showLibraryFilters = assets.length > 0 || hasActiveAssetFilters;
  const quadArtifact = quadTopologyArtifact(selectedAsset);
  const quadriflowBackend = runtime?.quadriflow_backend;
  const [installingBackend, setInstallingBackend] = useState(false);
  const isRunning = isActiveJob(job);
  const recentJobs = useMemo(() => mergeJobRecords(jobSnapshot, job ? [job] : []), [job, jobSnapshot]);
  const pendingGenerationJobs = recentJobs.filter(saved => isGenerationOperation(saved.operation) && isActiveJob(saved));
  const selectedPendingJob = pendingSelection ? recentJobs.find(saved => saved.id === pendingSelection.id) ?? pendingSelection : null;
  const retopologyAssetId = inspector === 'mesh.retopology' && !settingsSection
    && connection === 'connected' && !isRunning && !pendingGenerationJobs.some(saved => localGenerations.current.has(saved.id))
    && validQuadTarget && selectedAsset?.kind === 'mesh' ? selectedAsset.id : null;
  const { checking: checkingRetopology, report: retopologyCheck } = useRetopologyPreflight(api, retopologyAssetId, requestedQuads);
  const referenceNames = new Map(assets.map(asset => [asset.id, asset.name]));
  if (generationSource) referenceNames.set(generationSource.id, generationSource.name);
  const visiblePendingGenerationJobs = pendingGenerationJobs.filter(saved => saved.id === pendingSelection?.id
    || (assetFilters.origin !== 'original' && generationJobName(saved, referenceNames).toLowerCase().includes(assetFilters.query.toLowerCase())));
  const currentDetails = detailsTarget ? recentJobs.find(saved => saved.id === detailsTarget.id) ?? detailsTarget : null;
  const localActivities: StudioActivity[] = [
    ...(settingsActivity ? [settingsActivity] : []),
    ...(importProgress ? [{
      title: 'Import assets',
      stage: `Importing ${importProgress.current} of ${importProgress.total}: ${importProgress.fileName}`,
      startedAt: importStartedAt.current,
    }] : []),
    ...(installingBackend ? [{
      title: 'Install QuadriFlow',
      stage: 'Installing and checking the backend',
      startedAt: backendInstallStartedAt.current,
    }] : []),
    ...(downloading ? [{
      title: 'Export asset',
      stage: 'Reading the local artifact',
      startedAt: exportStartedAt.current,
    }] : []),
  ];
  const generationProfiles = useMemo(() => selectableGenerationProfiles(runtime), [runtime]);
  const selectedGenerationProfile = generationProfiles.find(profile => profile.modelId === generationModelId)
    ?? generationProfiles.find(profile => profile.ready) ?? generationProfiles[0];
  const inputModes = selectedGenerationProfile?.input_modes;
  const inputMode = inputModes?.includes(preferredInputMode) ? preferredInputMode : inputModes?.[0] ?? 'image';
  const viewMode = inputMode === 'image' && selectedGenerationProfile?.multiviewReady && selectedGenerationProfile.view_modes.includes(preferredViewMode)
    ? preferredViewMode : 'single';
  const posedViews = viewMode === 'single' ? [] : posedDrafts[viewMode] ?? defaultViews(viewMode);
  const posedInput = viewMode === 'single' ? null : buildPosedInput(viewMode, posedViews, selectedGenerationProfile?.max_views ?? 1);
  const generationAssetId = viewMode === 'single' ? generationSource?.id : posedInput?.references[0].asset_id;
  const prompt = generationPrompt.trim();
  const invalidPrompt = generationPrompt.includes('\0') || new TextEncoder().encode(generationPrompt).length > MAX_PROMPT_BYTES;
  const generationInput: GenerationInput | null = inputMode === 'text'
    ? prompt && !invalidPrompt ? { kind: 'text', prompt } : null
    : generationAssetId ? { kind: 'image', asset_id: generationAssetId } : null;
  const generationDraft = selectedGenerationProfile ? generationDrafts[selectedGenerationProfile.id] ?? {} : {};
  const generationPayload = selectedGenerationProfile
    ? generationParameters(selectedGenerationProfile, generationDraft, viewMode !== 'single') : null;
  function openSettings(section: SettingsSection, backendId?: string) {
    setSettingsBackend(backendId ?? null);
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

  function openJobs(jobId?: string) {
    if (settingsSection) {
      window.history.pushState(null, '', '/');
      setSettingsSection(null);
    }
    setFocusedJobId(jobId ?? null);
    sidebar.showPanel('jobs');
  }

  const publishInventoryAsset = useCallback((asset: Asset) => {
    const filters = assetFiltersRef.current;
    const current = filters.allVersions || !asset.model || asset.model.current_asset_id === asset.id;
    if (!current || !matchesAssetBrowserFilters(asset, filters)) return;
    const key = inventoryAssetKey(asset, filters);
    setInventoryAssets(items => {
      if (items.some(item => inventoryAssetKey(item, filters) === key)) {
        return items.map(item => inventoryAssetKey(item, filters) === key ? asset : item);
      }
      return key === asset.id ? [asset, ...items] : items;
    });
  }, []);

  useEffect(() => {
    const observed = new Map<string, Job['state']>();
    let newestGenerationResult: Job | undefined;
    for (const saved of recentJobs) {
      const prior = observedJobStates.current.get(saved.id);
      observed.set(saved.id, saved.state);
      if (!prior || prior === saved.state || saved.id === jobRef.current?.id) continue;
      const operation = isGenerationOperation(saved.operation) ? generationLabel(saved.parameters?.input_mode) : studioOperationLabel(saved.operation);
      const input = saved.input_asset_name ?? undefined;
      if (saved.state === 'running' && prior === 'queued') notify('info', `${operation} started.`, input);
      if (saved.state === 'succeeded') {
        notify('success', `${operation} complete.`, input);
        if (localGenerations.current.delete(saved.id) && saved.result?.asset_id) newestGenerationResult ??= saved;
      } else if (saved.state === 'failed') {
        localGenerations.current.delete(saved.id);
        notify('error', `${operation} failed.`, saved.error?.message ?? input);
      } else if (saved.state === 'cancelled') {
        localGenerations.current.delete(saved.id);
        notify('info', cancelJobNotice(saved), input);
      }
    }
    if (newestGenerationResult) setGenerationResult(newestGenerationResult);
    observedJobStates.current = observed;
  }, [recentJobs]);

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
      setInventoryAssets(current => {
        if (!append) return page.items;
        const pageKeys = new Set(page.items.map(asset => inventoryAssetKey(asset, filters)));
        return [...current.filter(asset => !pageKeys.has(inventoryAssetKey(asset, filters))), ...page.items];
      });
      setAssets((current) => {
        const selectedPair = comparisonPairRef.current
          ?? assetComparisonPairForSelection(current, selectedIdRef.current, null);
        const preservedIds = [
          selectedPair?.inputAssetId,
          selectedPair?.resultAssetId,
          selectedIdRef.current,
          current.find(asset => asset.id === selectedIdRef.current)?.model?.current_asset_id,
        ];
        const pageIds = new Set(page.items.map((asset) => asset.id));
        const retained = current.filter((asset) =>
          !pageIds.has(asset.id) && (append || preservedIds.includes(asset.id)));
        return mergeCachedAssets(page.items, retained);
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

  const refreshRecentJobs = useCallback((): Promise<boolean> => {
    if (jobFeedInFlight.current) return jobFeedInFlight.current;
    const requestId = recentJobsRequestGuard.current.begin();
    const request = api.jobs(JOB_PAGE_SIZE, { includeActive: true }).then(async jobs => {
      const known = new Set(jobs.map(saved => saved.id));
      const tracked = new Set(localGenerations.current);
      if (pendingSelectionRef.current) tracked.add(pendingSelectionRef.current.id);
      const missing = [...tracked].filter(id => !known.has(id));
      if (missing.length > 0) {
        const fetched = await Promise.all(missing.map(id => api.job(id).catch(() => null)));
        jobs = [...jobs, ...fetched.filter((saved): saved is Job => saved !== null)];
      }
      if (!recentJobsRequestGuard.current.isCurrent(requestId)) return false;
      setJobSnapshot(current => {
        const retained = new Set(jobs.map(saved => saved.id));
        const merged = mergeJobRecords(jobs, current.filter(saved => retained.has(saved.id) || tracked.has(saved.id)
          || localGenerations.current.has(saved.id) || saved.id === jobRef.current?.id));
        return [...merged.filter(isActiveJob), ...merged.filter(saved => !isActiveJob(saved)).slice(0, JOB_PAGE_SIZE)];
      });
      setJobFeedLoaded(true);
      for (const saved of jobs) {
        const assetId = saved.state === 'succeeded' ? saved.result?.asset_id : undefined;
        if (!assetId || observedJobResults.current.has(assetId)) continue;
        if (saved.id === jobRef.current?.id) {
          observedJobResults.current.add(assetId);
          continue;
        }
        if (saved.id === pendingSelectionRef.current?.id || assetFiltersRef.current.kind !== 'mesh') continue;
        observedJobResults.current.add(assetId);
        void api.derivedAsset(assetId).then(asset => {
          if (asset.id !== assetId) return;
          publishInventoryAsset(asset);
          setAssets(current => mergeCachedAssets([asset], current));
        }).catch(() => { observedJobResults.current.delete(assetId); });
      }
      return true;
    }).catch(cause => {
      if (!recentJobsRequestGuard.current.isCurrent(requestId)) return false;
      const message = cause instanceof Error ? `Could not refresh recent job records. ${cause.message}` : 'Could not refresh recent job records.';
      throw new Error(message);
    }).finally(() => {
      if (jobFeedInFlight.current === request) jobFeedInFlight.current = null;
    });
    jobFeedInFlight.current = request;
    return request;
  }, [api, publishInventoryAsset]);

  useEffect(() => {
    if (connection !== 'connected') return;
    let timer: number;
    let stopped = false;
    let unavailable = false;
    const poll = async () => {
      window.clearTimeout(timer);
      if (document.visibilityState === 'visible') {
        try {
          if (await refreshRecentJobs()) unavailable = false;
        } catch (cause) {
          if (!stopped && !unavailable) notify('error', cause instanceof Error ? cause.message : 'Could not refresh job history.');
          unavailable = true;
        }
      }
      if (!stopped) {
        window.clearTimeout(timer);
        timer = window.setTimeout(poll, 2000);
      }
    };
    const onVisibilityChange = () => { if (document.visibilityState === 'visible') void poll(); };
    timer = window.setTimeout(poll, 2000);
    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => {
      stopped = true;
      window.clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  }, [connection, refreshRecentJobs]);

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
        setJobSnapshot(current => mergeJobRecords([response], current));
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
      setJobSnapshot(current => mergeJobRecords([latest], current));

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
          : `Last saved state: ${latest.state} · ${jobStage(latest)}. No work was resumed.`);
        return;
      }

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
        publishInventoryAsset(resultAsset);
        setAssets(current => mergeCachedAssets([resultAsset], current));
        selectAssetId(resultAsset.id);
      }
    } catch (cause) {
      if (!historySelectionGuard.current.isCurrent(requestId)) return;
      notify('error', cause instanceof Error ? cause.message : 'Could not load this saved job.');
    }
  }

  const installQuadriflowBackend = useCallback(async () => {
    if (!quadriflowBackend?.install_available || installingBackend) return;
    backendInstallStartedAt.current = Date.now();
    setInstallingBackend(true);
    notify('info', 'Installing QuadriFlow.');
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
    recentJobsRequestGuard.current.invalidate();
    jobFeedInFlight.current = null;
    assetPageRequestGuard.current.invalidate();
    setAssetPageLoading(true);
    setComparisonLoadingSide(null);
    setComparisonPair(null);
    setConnection('connecting');
    try {
      const health = await api.health();
      if (health.service !== 'meshharbor' || health.status !== 'ok') throw new Error('This does not look like a MeshHarbor server.');
      const [runtimeInfo, operationList, assetPage, jobList] = await Promise.all([
        api.runtime(), api.operations(), api.assetPage(ASSET_PAGE_SIZE, null, toAssetPageFilters(assetFiltersRef.current)),
        api.jobs(JOB_PAGE_SIZE, { includeActive: true }),
      ]);
      setRuntime(runtimeInfo);
      setOperations(operationList);
      setInventoryAssets(assetPage.items);
      setAssets((current) => {
        const selectedPair = comparisonPairRef.current
          ?? assetComparisonPairForSelection(current, selectedIdRef.current, null);
        const pageIds = new Set(assetPage.items.map((asset) => asset.id));
        const preservedIds = new Set([
          selectedPair?.inputAssetId,
          selectedPair?.resultAssetId,
          selectedIdRef.current,
        ]);
        return mergeCachedAssets(assetPage.items, current.filter((asset) => preservedIds.has(asset.id) && !pageIds.has(asset.id)));
      });
      setAssetNextCursor(assetPage.next_cursor);
      observedJobResults.current = new Set(jobList.flatMap(saved =>
        saved.state === 'succeeded' && saved.result?.asset_id ? [saved.result.asset_id] : []));
      setJobSnapshot(jobList);
      setJobFeedLoaded(true);
      setSelectedId((current) => {
        const next = pendingSelectionRef.current ? null : current && (assetPage.items.some((item) => item.id === current)
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
      notify('error', 'Could not connect to the local engine.', message);
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

  const selectAssetId = useCallback((assetId: string | null) => {
    assetSelectionInteractionRevision.current += 1;
    selectedIdRef.current = assetId;
    pendingSelectionRef.current = null;
    setPendingSelection(null);
    setSelectedId(assetId);
  }, []);

  function selectPendingModel(pending: Job, reveal = false) {
    invalidateHistoryRequests();
    selectAssetId(null);
    pendingSelectionRef.current = pending;
    setPendingSelection(pending);
    setComparisonPair(null);
    setReportJob(null);
    if (reveal) {
      if (assetFiltersRef.current.kind !== 'mesh' || hasActiveAssetFilters || assetSearchInputRef.current) {
        applyAssetBrowserFilters(EMPTY_ASSET_BROWSER_FILTERS);
      }
      sidebar.showPanel('inventory');
    } else if (sidebar.narrow) sidebar.setOpen(false);
  }

  const pendingPreviewJobId = selectedPendingJob?.id;
  const pendingPreviewAssetId = selectedPendingJob?.state === 'succeeded' ? selectedPendingJob.result?.asset_id : undefined;
  useEffect(() => {
    if (!pendingPreviewJobId || !pendingPreviewAssetId || connection !== 'connected') return;
    let stopped = false;
    void api.derivedAsset(pendingPreviewAssetId).then(asset => {
      if (asset.id !== pendingPreviewAssetId) throw new Error('The generation result did not match the selected job.');
      if (stopped || pendingSelectionRef.current?.id !== pendingPreviewJobId) return;
      observedJobResults.current.add(asset.id);
      publishInventoryAsset(asset);
      setAssets(current => mergeCachedAssets([asset], current));
      selectAssetId(asset.id);
    }).catch(cause => {
      if (!stopped && pendingSelectionRef.current?.id === pendingPreviewJobId) {
        notify('error', 'Could not open the generated model.', cause instanceof Error ? cause.message : undefined);
      }
    });
    return () => { stopped = true; };
  }, [api, connection, pendingPreviewAssetId, pendingPreviewJobId, publishInventoryAsset, selectAssetId]);

  useEffect(() => {
    if (!job || !['queued', 'running', 'validating', 'cancelling'].includes(job.state)) return;
    let stopped = false;
    let timer = 0;
    let consecutiveFailures = 0;
    const poll = async () => {
      try {
        const latest = await api.job(job.id);
        if (stopped || jobRef.current?.id !== job.id || !isActiveJob(jobRef.current)) return;
        if (jobRef.current.state === 'cancelling' && isActiveJob(latest) && latest.state !== 'cancelling') {
          timer = window.setTimeout(poll, 700);
          return;
        }
        consecutiveFailures = 0;
        if (latest.state === 'running' && jobRef.current.state === 'queued') {
          notify('info', `${studioOperationLabel(latest.operation)} started.`);
        }
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
            notify('success', latest.operation === 'runtime.models.install'
              ? (latest.parameters?.backend_id?.startsWith('geosam2-')
                ? 'GeoSAM2 backend and weights installed for the command-line prototype.'
                : 'Model installed and ready for offline generation.')
              : latest.operation === 'runtime.environment.setup'
              ? 'Shared environment is ready. Sign in to Hugging Face in Connections to install models.'
              : latest.operation === 'runtime.pixal3d.setup'
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
            const advance = shouldAutoSelectComparisonResult(pair, selectedIdRef.current);
            setComparisonPair(pair);
            setReportJob(latest);
            observedJobResults.current.add(completedAsset.id);
            publishInventoryAsset(completedAsset);
            setAssets(current => mergeCachedAssets([completedAsset], current));
            if (advance) {
              selectAssetId(completedAsset.id);
            }
          }
          const resultReport = latest.result?.report ?? {};
          if (latest.operation === 'mesh.cleanup') {
            const removed = Math.max(
              0,
              Number(resultReport.triangles_before ?? 0) - Number(resultReport.triangles_after ?? 0),
            );
            const simplified = Number(resultReport.simplified_triangles_removed ?? 0);
            notify('success', removed > 0
              ? `Cleanup complete. Removed ${removed.toLocaleString()} triangles (${simplified.toLocaleString()} by simplification).`
              : 'Cleanup complete. No removable triangles were found.');
          } else if (latest.operation === 'mesh.voxel_remesh') {
            notify('success', 'Voxel Remesh complete. The new version contains geometry only.');
          } else if (latest.operation === 'render.optimize') {
            const before = Number(resultReport.vertex_cache_misses_before ?? 0);
            const after = Number(resultReport.vertex_cache_misses_after ?? 0);
            notify('success', `Render optimization complete. Estimated vertex-cache misses: ${before.toLocaleString()} → ${after.toLocaleString()}.`);
          } else if (latest.operation === 'mesh.compress') {
            const saved = Number(resultReport.bytes_saved ?? 0);
            notify('success', `Compression complete. Saved ${formatSize(saved)}. The exported GLB requires KHR_meshopt_compression support.`);
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
            notify('success', 'Mesh simplification complete. Your original file is unchanged.');
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
          notify('error', `${studioOperationLabel(latest.operation)} failed.`, latest.error?.message);
          return;
        }
        if (latest.state === 'cancelled') {
          setJob(latest);
          notify('success', cancelJobNotice(latest));
          return;
        }
        setJob(latest);
        timer = window.setTimeout(poll, 700);
      } catch (cause) {
        if (!stopped) {
          consecutiveFailures += 1;
          const delay = jobPollRetryDelay(consecutiveFailures);
          if (consecutiveFailures === 1) {
            notify('warning', 'Job status unavailable. Retrying…', cause instanceof Error ? cause.message : undefined);
          }
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
    invalidateHistoryRequests();
    const pair = assetComparisonPairForSelection(mergeCachedAssets([asset], assets), asset.id, comparisonPair);
    setComparisonPair(pair);
    selectAssetId(asset.id);
    setReportJob(null);
  }

  function showModelVersion(asset: Asset) {
    publishInventoryAsset(asset);
    setAssets(current => mergeCachedAssets([asset], current));
    selectAsset(asset);
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
    importStartedAt.current = Date.now();
    const startNotice = window.setTimeout(() => notify('info', 'Import in progress.'), 500);
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
      window.clearTimeout(startNotice);
      importBatchInFlight.current = false;
      setImportProgress(null);
      setBusy(false);
    }
  }

  async function simplify() {
    if (!selectedAsset || !canSimplify) return;
    invalidateHistoryRequests();
    setBusy(true);
    setReportJob(null);
    setComparisonPair(null);
    try {
      const nextJob = await api.createSimplifyJob(selectedAsset.id, targetTriangles, maxError);
      setJob(nextJob);
      notify('info', `Mesh simplification ${nextJob.state === 'queued' ? 'queued' : 'started'}.`);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not start simplification.');
    } finally {
      setBusy(false);
    }
  }

  async function runMeshOperation(operation: MeshOperationId, parameters: Record<string, number | boolean | string> = {}) {
    if (!selectedAsset || selectedAsset.kind !== 'mesh' || (operation === 'mesh.retopology' && !retopologyCheck)) return;
    invalidateHistoryRequests();
    setBusy(true);
    setReportJob(null);
    setComparisonPair(null);
    try {
      if (operation === 'mesh.retopology' && requestedQuads !== undefined) parameters.target_quads = requestedQuads;
      const nextJob = await api.createMeshOperationJob(selectedAsset.id, operation, parameters);
      setJob(nextJob);
      notify('info', `${studioOperationLabel(operation)} ${nextJob.state === 'queued' ? 'queued' : 'started'}.`);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not start mesh processing.');
    } finally {
      setBusy(false);
    }
  }

  async function generate() {
    if (!generationInput || !selectedGenerationProfile || !generationPayload || !canGenerate || busy || connection !== 'connected') return;
    setBusy(true);
    try {
      const nextJob = await api.createModelGenerationJob(generationInput, selectedGenerationProfile.id, {
        ...generationPayload,
        ...(posedInput ? { view_input: posedInput } : {}),
      });
      if (isActiveJob(nextJob)) localGenerations.current.add(nextJob.id);
      if (!observedJobStates.current.has(nextJob.id)) observedJobStates.current.set(nextJob.id, nextJob.state);
      setJobSnapshot(current => mergeJobRecords([nextJob], current));
      selectPendingModel(nextJob, true);
      const label = generationLabel(generationInput.kind);
      if (nextJob.state === 'succeeded') {
        setGenerationResult(nextJob);
        notify('success', `${label} complete.`);
      } else if (nextJob.state === 'failed') {
        notify('error', `${label} failed.`, nextJob.error?.message);
      } else if (nextJob.state === 'cancelled') {
        notify('info', cancelJobNotice(nextJob));
      } else {
        notify('info', `${label} ${nextJob.state === 'queued' ? 'queued' : 'started'}.`,
          nextJob.input_asset_name ?? (generationInput.kind === 'image' ? referenceNames.get(generationInput.asset_id) : undefined));
      }
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not start generation.');
    } finally {
      setBusy(false);
    }
  }

  async function startRuntimeAction(start: () => Promise<Job>) {
    if (busy || isRunning || connection !== 'connected') return;
    invalidateHistoryRequests();
    setBusy(true);
    try {
      const nextJob = await start();
      setJob(nextJob);
      notify('info', `${studioOperationLabel(nextJob.operation)} ${nextJob.state === 'queued' ? 'queued' : 'started'}.`);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not start the runtime operation.');
    } finally {
      setBusy(false);
    }
  }

  function runRuntimeAction(backend: RuntimeBackendId, action: RuntimeAction) {
    void startRuntimeAction(() => api.installCatalogModel(
      backend, action === 'install-gated', action === 'multiview' ? 'multiview' : 'base',
    ));
  }

  async function downloadBlob(load: () => Promise<Blob>, fileName: (blob: Blob) => string, label: string) {
    if (downloadInFlight.current || connection !== 'connected') return;
    downloadInFlight.current = true;
    exportStartedAt.current = Date.now();
    setDownloading(true);
    const startNotice = window.setTimeout(() => notify('info', 'Preparing export.', label), 500);
    try {
      const blob = await load();
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = fileName(blob);
      anchor.click();
      notify('success', `Export started: ${anchor.download}`);
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not download this asset.');
    } finally {
      window.clearTimeout(startNotice);
      downloadInFlight.current = false;
      setDownloading(false);
    }
  }

  function downloadResult(artifact?: Artifact) {
    if (!selectedAsset) return;
    const filename = artifact ? `${selectedAsset.name.replace(/\.[^.]+$/, '')}-quads.obj` : selectedAsset.name;
    return downloadBlob(() => artifact ? api.downloadArtifact(artifact) : api.artifact(selectedAsset), () => filename, filename);
  }

  async function openStoredAsset(id: string) {
    if (!id || busy || connection !== 'connected') return;
    if (!/^asset_[a-z0-9_-]+$/i.test(id)) {
      notify('error', 'Enter a stored asset ID starting with asset_.');
      return;
    }
    const revision = assetSelectionInteractionRevision.current;
    setBusy(true);
    try {
      const asset = await api.derivedAsset(id);
      if (assetSelectionInteractionRevision.current === revision) showModelVersion(asset);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not open this asset.');
    } finally {
      setBusy(false);
    }
  }

  function downloadStoredArtifact(id: string) {
    if (!/^artifact_[a-z0-9_-]+$/i.test(id)) {
      notify('error', 'Enter a stored artifact ID starting with artifact_.');
      return;
    }
    const extensions: Record<string, string> = { 'model/gltf-binary': '.glb', 'model/obj': '.obj', 'image/png': '.png', 'image/jpeg': '.jpg' };
    return downloadBlob(() => api.downloadArtifactById(id), blob => id + (extensions[blob.type] ?? ''), id);
  }

  function requestStop(target: Job, trigger: HTMLButtonElement) {
    stopTrigger.current = trigger;
    setStopTarget(target);
  }

  function showJobDetails(target: Job, trigger: HTMLButtonElement) {
    detailsTrigger.current = trigger;
    setDetailsTarget(target);
  }

  function focusPanelHeading() {
    if (!settingsSection && sidebar.open && sidebar.panel === 'jobs' && sidebar.narrow) {
      document.querySelector<HTMLElement>('#workbench-sidebar-drawer [data-slot="sheet-title"]')?.focus();
      return;
    }
    const headingId = settingsSection ? 'settings-heading' : sidebar.open && sidebar.panel === 'jobs'
      ? 'workbench-sidebar-title' : 'workbench-inspector-title';
    document.getElementById(headingId)?.focus();
  }

  async function retryJob() {
    if (!retryTarget || busy || isRunning || connection !== 'connected') return;
    setBusy(true);
    try {
      const next = await api.retryJob(retryTarget.id, retryTarget.operation.startsWith('runtime.'));
      if (isGenerationOperation(next.operation)) {
        localGenerations.current.add(next.id);
        observedJobStates.current.set(next.id, next.state);
        setJobSnapshot(current => mergeJobRecords([next], current));
      } else {
        setJob(next);
      }
      setFocusedJobId(next.id);
      setRetryTarget(null);
      notify('info', `${studioOperationLabel(next.operation)} ${next.state === 'queued' ? 'queued' : 'started'} again.`);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not retry this job.');
    } finally {
      setBusy(false);
    }
  }

  async function stopJob() {
    if (!stopTarget || stoppingJob) return;
    setStoppingJob(true);
    try {
      const queuedOnly = stopTarget.state === 'queued';
      const updated = await api.cancelJob(stopTarget.id, queuedOnly);
      if (jobRef.current?.id === updated.id && isActiveJob(jobRef.current)) {
        if (updated.state === 'cancelled' || updated.state === 'cancelling') setJob(updated);
      }
      if (isGenerationOperation(updated.operation) && updated.state === 'cancelled') {
        localGenerations.current.delete(updated.id);
      }
      if (updated.state === 'cancelled') observedJobStates.current.set(updated.id, updated.state);
      setJobSnapshot(current => mergeJobRecords([updated], current));
      const alreadyStarted = queuedOnly && updated.state !== 'queued' && updated.state !== 'cancelled';
      notify(updated.state === 'cancelled' ? 'success' : !alreadyStarted && updated.state === 'cancelling' ? 'info' : 'warning',
        alreadyStarted ? 'The job already started and was not removed.' : cancelJobNotice(updated), stopTarget.input_asset_name ?? undefined);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not stop this job.');
    } finally {
      setStoppingJob(false);
      setStopTarget(null);
    }
  }

  const triangleCount = Number(selectedAsset?.metadata.triangles ?? 0);
  const triangleInputMax = triangleCount ? Math.min(10_000_000, triangleCount - 1) : 10_000_000;
  const triangleSliderMax = Math.max(2, triangleCount ? triangleInputMax : 20_000);
  const validTriangleLimit = Number.isInteger(targetTriangles) && targetTriangles >= 1 && targetTriangles <= triangleInputMax;
  const canSimplify = selectedAsset?.kind === 'mesh' && validTriangleLimit && validMaxError
    && operations.some(operation => operation.id === 'mesh.simplify' && operation.implemented);
  const canGenerate = !!operations.find((operation) => operation.id === 'model.generate')?.implemented
    && selectedGenerationProfile?.ready === true;
  const isComparingResult = selectedComparisonPair
    ? selectedComparisonPair.resultAssetId === selectedId
    : Boolean(selectedAsset?.source_asset_id || selectedAsset?.model?.generated);
  const activeView: AssetComparisonSide = isComparingResult ? 'result' : 'source';
  const openInspector = inspector;
  const hasSelectedMesh = selectedAsset?.kind === 'mesh';
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
  const meshTool = meshOperationOptions.find(tool => tool.id === openInspector);
  const isRetopology = meshTool?.id === 'mesh.retopology';
  const isMeshToMesh = meshTool?.id === 'mesh.retopology_lato2';
  const isVoxelRemesh = meshTool?.id === 'mesh.voxel_remesh';
  const meshParameters = meshTool?.id === 'uv.unwrap' || meshTool?.id === 'render.optimize'
    ? meshParameterFields[meshTool.id].map(field => {
      const draft = meshParameterDrafts[field.name] ?? field.initial;
      const value = Number(draft);
      const valid = draft !== '' && Number.isFinite(value) && value >= field.min && value <= field.max
        && (field.step !== '1' || Number.isInteger(value));
      return { ...field, draft, value, valid };
    }) : [];
  const meshToolAvailable = operations.some(operation => operation.id === meshTool?.id && operation.implemented);
  const canPrepareRetopology = hasSelectedMesh && !busy && !checkingRetopology && connection === 'connected' && !isRunning;
  const processingTool = isRunning && job.operation === meshTool?.id;
  const inspectorTitle = meshTool?.title ?? (openInspector === 'generation' ? 'Generate model' : 'Optimize mesh');
  const exportIsPrimary = shouldHighlightExport(activeView, isRunning, connection === 'connected');

  function renderAssetCard(asset: Asset) {
    return <AssetGalleryCard key={inventoryAssetKey(asset, assetFilters)} asset={asset} loadPreview={loadPreview} showVersion={assetFilters.allVersions}
      selected={selectedAsset !== null && inventoryAssetKey(asset, assetFilters) === inventoryAssetKey(selectedAsset, assetFilters)}
      onDragImage={image => {
        draggedImage.current = image;
        if (image && !busy && !isRunning) setInspector('generation');
      }}
      onSelect={() => { selectAsset(asset); if (sidebar.narrow) sidebar.setOpen(false); }} />;
  }

  function attachInventoryReference(id: string, index?: number) {
    const image = draggedImage.current;
    if (!image || image.id !== id || image.kind !== 'image' || busy || isRunning) return;
    draggedImage.current = null;
    if (viewMode === 'single') setGenerationSource(image);
    else if (index !== undefined) setPosedDrafts(current => ({
      ...current,
      [viewMode]: (current[viewMode] ?? defaultViews(viewMode)).map((view, currentIndex) =>
        currentIndex === index ? { ...view, image } : view),
    }));
  }

  function singleReferencePicker() {
    return <SingleReference image={generationSource} load={loadPreview} disabled={busy || isRunning}
      onPick={files => void importFiles(files, 'reference')} onDropImage={attachInventoryReference} onClear={() => setGenerationSource(null)} />;
  }

  function inspectorBody() {
    return <>
      {(openInspector === 'optimize' || meshTool) && (
        <div className="grid min-w-0 gap-1 border-b px-4 py-3 text-xs">
          <span className="text-muted-foreground">Selected model</span>
          {hasSelectedMesh ? <span className="truncate font-medium" title={selectedAsset.name}>{selectedAsset.name}</span>
            : <Button variant="outline" size="sm" className="justify-self-start" onClick={() => {
              if (assetFilters.kind !== 'mesh') applyAssetBrowserFilters(EMPTY_ASSET_BROWSER_FILTERS);
              sidebar.showPanel('inventory');
            }}>Choose a 3D model</Button>}
        </div>
      )}
      {openInspector === 'optimize' && <div className="grid gap-4 p-4 text-xs/relaxed">
        <Field>
          <div className="flex items-center gap-1">
            <FieldLabel htmlFor="triangle-target">Triangle limit</FieldLabel>
            <HelpTip label="About the triangle limit">Set a limit below the source triangle count. If the result cannot meet that limit, no result is saved.</HelpTip>
          </div>
          <Input id="triangle-target" type="number" min="1" max={Math.max(1, triangleInputMax)} step="1" value={targetTriangles}
            aria-invalid={hasSelectedMesh && !validTriangleLimit} aria-describedby={hasSelectedMesh && !validTriangleLimit ? 'triangle-limit-description' : undefined}
            onChange={(event) => setTargetTriangles(Number(event.target.value))} disabled={!hasSelectedMesh || triangleInputMax < 1 || isRunning || busy} />
          {hasSelectedMesh && !validTriangleLimit && <FieldDescription id="triangle-limit-description">
            {triangleInputMax < 1 ? 'This mesh already has one triangle.'
              : triangleInputMax === 1 ? 'Enter 1 as the triangle limit.'
              : `Enter a whole number from 1 to ${triangleInputMax.toLocaleString()}.`}
          </FieldDescription>}
        </Field>
        <div className="flex items-center gap-2 text-muted-foreground [&>span]:shrink-0">
          <span>Smaller</span>
          <Slider className="min-w-0 flex-1" aria-label="Triangle limit" min={1} max={triangleSliderMax} step={1}
            value={[Math.max(1, Math.min(targetTriangles, triangleSliderMax))]} onValueChange={([value]) => setTargetTriangles(value)}
            disabled={!hasSelectedMesh || triangleInputMax <= 1 || isRunning || busy} />
          <span>More detail</span>
        </div>
        <Accordion type="single" collapsible>
          <AccordionItem value="shape-protection">
            <AccordionTrigger>Shape protection</AccordionTrigger>
            <AccordionContent>
              <div className="grid gap-3">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-1"><span>Error tolerance</span>
                    <HelpTip label="About error tolerance">Lower tolerance protects the shape but may prevent the mesh from reaching the triangle limit. The limit is never exceeded.</HelpTip>
                  </div>
                </div>
                <Slider aria-label="Quick geometric error adjustment" min={0.001} max={0.08} step={0.001}
                  value={[validMaxError ? Math.max(0.001, Math.min(0.08, maxError)) : 0.02]}
                  onValueChange={([value]) => setMaxErrorDraft(String(value))} disabled={!hasSelectedMesh || isRunning || busy} />
                <Input aria-label="Maximum geometric error" aria-invalid={!validMaxError} type="number" min="0" max="1" step="any"
                  value={maxErrorDraft} onChange={event => setMaxErrorDraft(event.currentTarget.value)} disabled={!hasSelectedMesh || isRunning || busy} />
                {!validMaxError && <FieldDescription>Enter an error tolerance from 0 to 1.</FieldDescription>}
              </div>
            </AccordionContent>
          </AccordionItem>
        </Accordion>
        <Button className="w-full" disabled={!canSimplify || busy || connection !== 'connected' || isRunning} onClick={() => void simplify()}>
          <SlidersHorizontalIcon aria-hidden="true" /> Optimize mesh
        </Button>
      </div>}

      {openInspector === 'generation' && <section className="grid gap-4 p-4 text-xs/relaxed" id="model-generation">
          {selectedGenerationProfile && <>
            <Field>
              <div className="flex items-center gap-1">
                <FieldLabel htmlFor="generation-model">Generation model</FieldLabel>
                <HelpTip label="About generation models">LATO.2 can be selected before setup, but generation needs a configured local runtime.</HelpTip>
              </div>
              <NativeSelect className="w-full" id="generation-model" value={selectedGenerationProfile.modelId}
                onChange={event => { setGenerationModelId(event.target.value); setPreferredViewMode('single'); }} disabled={busy || isRunning}>
                {generationProfiles.map(profile => <NativeSelectOption key={profile.modelId} value={profile.modelId}>
                  {profile.label}{!profile.ready ? ' (setup required)' : ''}
                </NativeSelectOption>)}
              </NativeSelect>
            </Field>
            {selectedGenerationProfile.output === 'geometry_glb' && <p className="text-muted-foreground">Output: geometry-only GLB (no textures).</p>}
            {!selectedGenerationProfile.ready && <div className="grid gap-2">
              <p role="status" className="text-muted-foreground">{selectedGenerationProfile.setupMessage ?? 'The local runtime needs setup.'}</p>
              <Button variant="outline" size="sm" className="self-start" onClick={() => openSettings('models')}>View model setup</Button>
            </div>}
            {selectedGenerationProfile.input_modes.length > 1 && <Field>
              <FieldLabel htmlFor="generation-input-mode">Input type</FieldLabel>
              <NativeSelect className="w-full" id="generation-input-mode" value={inputMode} disabled={busy || isRunning}
                onChange={event => {
                  const next = selectedGenerationProfile.input_modes.find(mode => mode === event.target.value);
                  if (next) setPreferredInputMode(next);
                }}>
                {selectedGenerationProfile.input_modes.map(mode => <NativeSelectOption key={mode} value={mode}>{inputModeLabels[mode]}</NativeSelectOption>)}
              </NativeSelect>
            </Field>}
            {inputMode === 'image' && selectedGenerationProfile.view_modes.length > 1 && <Field>
              <FieldLabel htmlFor="generation-view-mode">View mode</FieldLabel>
              <NativeSelect className="w-full" id="generation-view-mode" value={viewMode} disabled={busy || isRunning}
                onChange={event => {
                  const next = selectedGenerationProfile.view_modes.find(mode => mode === event.target.value);
                  if (next) setPreferredViewMode(next);
                }}>
                {selectedGenerationProfile.view_modes.map(mode => <NativeSelectOption key={mode} value={mode}
                  disabled={mode !== 'single' && !selectedGenerationProfile.multiviewReady}>{viewLabels[mode]}</NativeSelectOption>)}
              </NativeSelect>
              {!selectedGenerationProfile.multiviewReady && selectedGenerationProfile.multiviewInstallable && <>
                <Button variant="outline" size="sm" className="self-start" onClick={() => openSettings('models', selectedGenerationProfile.id)}>Set up multiview</Button>
              </>}
            </Field>}
            {inputMode === 'text' ? <Field>
              <FieldLabel htmlFor="generation-prompt">Text prompt</FieldLabel>
              <Textarea id="generation-prompt" value={generationPrompt} disabled={busy || isRunning}
                aria-invalid={invalidPrompt} onChange={event => setGenerationPrompt(event.currentTarget.value)} />
              {invalidPrompt && <p role="alert">Use at most {MAX_PROMPT_BYTES} UTF-8 bytes and no NUL characters.</p>}
            </Field> : viewMode === 'single'
              ? singleReferencePicker()
              : <PosedReferences mode={viewMode} views={posedViews} maxViews={selectedGenerationProfile.max_views} load={loadPreview} disabled={busy || isRunning}
                onPick={(files, index) => void importFiles(files, { mode: viewMode, maxViews: selectedGenerationProfile.max_views, index })}
                onDropImage={attachInventoryReference}
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
          {(!selectedGenerationProfile || selectedGenerationProfile.ready && !canGenerate) && <Empty className="border">
            <EmptyHeader>
              <EmptyTitle>No generation model is ready.</EmptyTitle>
              <EmptyDescription>You can still open and edit existing GLBs.</EmptyDescription>
            </EmptyHeader>
            <EmptyContent><Button variant="secondary" onClick={() => openSettings('models')}>Open model setup</Button></EmptyContent>
          </Empty>}
          {!selectedGenerationProfile && singleReferencePicker()}
        </section>}

      {meshTool && <section className="grid gap-4 p-4 text-xs/relaxed" aria-label={meshTool.title}>
        <p className="text-muted-foreground">{meshTool.description}</p>
        {isRetopology && !meshToolAvailable && quadriflowBackend && (
          <p className="text-muted-foreground">{quadriflowBackend.message}</p>
        )}
        {isMeshToMesh ? <MeshToMeshSettings models={selectableMeshToMeshModels(runtime)} running={processingTool}
          disabled={!hasSelectedMesh || busy || connection !== 'connected' || isRunning}
          onRun={parameters => void runMeshOperation('mesh.retopology_lato2', parameters)}
          onSetup={() => openSettings('models')} /> : <div className="grid gap-2">
          {meshParameters.map(field => <Field key={field.name}>
            <FieldLabel htmlFor={`mesh-parameter-${field.name}`}>{field.label}</FieldLabel>
            <Input id={`mesh-parameter-${field.name}`} type="number" min={field.min} max={field.max} step={field.step}
              value={field.draft} aria-invalid={!field.valid} disabled={!hasSelectedMesh || busy || isRunning}
              onChange={event => {
                const draft = event.currentTarget.value;
                setMeshParameterDrafts(current => ({ ...current, [field.name]: draft }));
              }} />
            {!field.valid && <FieldDescription>Enter {field.step === '1' ? 'a whole number' : 'a number'} from {field.min} to {field.max}.</FieldDescription>}
          </Field>)}
          {isVoxelRemesh && <Field>
            <FieldLabel htmlFor="voxel-resolution">Voxel resolution</FieldLabel>
            <Input id="voxel-resolution" type="number" min={MIN_VOXEL_RESOLUTION} max={MAX_VOXEL_RESOLUTION} step="1"
              value={voxelResolution} aria-invalid={!validVoxelResolution} aria-describedby="voxel-resolution-hint"
              onChange={event => setVoxelResolution(event.currentTarget.value)} disabled={busy || isRunning} />
            <FieldDescription id="voxel-resolution-hint">
              {MIN_VOXEL_RESOLUTION}–{MAX_VOXEL_RESOLUTION}. Higher values can preserve more detail but may leave gaps unresolved.
            </FieldDescription>
          </Field>}
          {isRetopology && <>
            <Field orientation="horizontal">
              <Checkbox id="use-quad-target" checked={quadTargetEnabled}
                onCheckedChange={checked => setQuadTargetEnabled(checked === true)} disabled={busy || isRunning} />
              <FieldLabel htmlFor="use-quad-target">Set target quads</FieldLabel>
              <HelpTip label="About the quad target">Leave off to let QuadriFlow choose the mesh density. A custom target is approximate.</HelpTip>
            </Field>
            {quadTargetEnabled && <Field>
              <div className="flex items-center gap-1">
                <FieldLabel htmlFor="quad-target">Target quads</FieldLabel>
                <span id="quad-range" className="text-muted-foreground">4–50,000</span>
              </div>
              <Input id="quad-target" type="number" min="4" max="50000" step="1"
                value={targetQuads} aria-invalid={!validQuadTarget} aria-describedby="quad-range"
                onChange={event => setTargetQuads(event.currentTarget.value)} disabled={busy || isRunning} />
            </Field>}
            {(checkingRetopology || retopologyCheck) && <div className="flex items-center gap-1 text-muted-foreground">
              <span role="status" className="flex items-center gap-1">
                {checkingRetopology ? <><Spinner aria-hidden="true" /> Checking input…</>
                  : retopologyCheck?.appearance_projection_required ? 'Textured input passed' : 'Geometry-only input passed'}
              </span>
              {retopologyCheck && <HelpTip label="About the input check">
                {retopologyCheck.input_triangles.toLocaleString()} triangles. The solver did not run.
                {retopologyCheck.coincident_boundary_vertices_joined && ' Exact seams can be joined without changing the source.'}
              </HelpTip>}
            </div>}
            <Button variant="outline" className="w-full"
              disabled={!canPrepareRetopology || !operations.some(operation => operation.id === 'mesh.cleanup' && operation.implemented)}
              onClick={() => void runMeshOperation('mesh.cleanup', { for_retopology: true })}
            >
              Run Cleanup first
            </Button>
            <Button variant="outline" className="w-full" aria-describedby="retopology-voxel-hint"
              disabled={!canPrepareRetopology || !operations.some(operation => operation.id === 'mesh.voxel_remesh' && operation.implemented)}
              onClick={() => void runMeshOperation('mesh.voxel_remesh')}
            >
              Run Voxel Remesh
            </Button>
            <p id="retopology-voxel-hint" className="text-muted-foreground">
              Voxel Remesh rebuilds the surface as a geometry-only version at resolution {DEFAULT_VOXEL_RESOLUTION}.
            </p>
          </>}
          {!processingTool && (
            <Button
              className="w-full"
              disabled={!hasSelectedMesh || !meshToolAvailable || busy || connection !== 'connected' || isRunning
                || (isRetopology && !retopologyCheck) || (isVoxelRemesh && !validVoxelResolution)
                || meshParameters.some(field => !field.valid)}
              onClick={() => void runMeshOperation(meshTool.id, isVoxelRemesh ? { resolution: requestedVoxelResolution }
                : Object.fromEntries(meshParameters.map(field => [field.name, field.value])))}
            >
              Run {meshTool.title}
            </Button>
          )}
        </div>}
      </section>}

      {openInspector === 'mesh.retopology' && <Disclosure className="mx-4 mb-4 border-y py-2 text-xs/relaxed" title="Retopology availability">
        <div className="my-2 flex flex-wrap items-center justify-between gap-x-2">
          <span>QuadriFlow backend</span>
          <div className="flex items-center gap-1 text-muted-foreground">
            <span>{quadriflowBackend?.status.replaceAll('_', ' ') ?? 'Connect engine'}</span>
            <HelpTip label="About the QuadriFlow backend">
              {quadriflowBackend ? <>{quadriflowBackend.message} {quadriflowBackend.next_step}</>
                : 'Retopology needs an available QuadriFlow backend.'}
            </HelpTip>
          </div>
        </div>
        {!quadriflowBackend?.available && <Button onClick={() => void installQuadriflowBackend()} disabled={!quadriflowBackend?.install_available || installingBackend}>{installingBackend ? 'Installing…' : quadriflowBackend?.install_available ? 'Install QuadriFlow' : 'Install unavailable'}</Button>}
      </Disclosure>}
    </>;
  }

  const toolRail = <div role="group" aria-label="Model tools" data-slot="model-tools" className="h-full overflow-y-auto">
    <SidebarMenu className="p-1.5">
      {([
        { key: 'generation', label: 'New model', icon: ImageSquareIcon },
        { key: 'optimize', label: 'Optimize', icon: SlidersHorizontalIcon },
        ...meshOperationOptions.map(tool => ({ key: tool.id, label: tool.label ?? tool.title, icon: tool.icon })),
      ] as const).map(({ key, label, icon: Icon }) => (
        <SidebarMenuItem key={key}>
          <SidebarMenuButton type="button" isActive={openInspector === key} aria-label={label}
            tooltip={{ children: label, hidden: false, sideOffset: 8 }}
            className="size-9 justify-center p-0 data-active:text-primary [&_svg]:size-5"
            aria-current={openInspector === key ? true : undefined} aria-controls="workbench-inspector"
            onClick={() => setInspector(key)}>
            <Icon aria-hidden="true" />
          </SidebarMenuButton>
        </SidebarMenuItem>
      ))}
    </SidebarMenu>
  </div>;
  const showGenerateAction = canGenerate;
  const inspectorContent = <>
    <ScrollArea key={openInspector} type="auto" className="min-h-0 min-w-0 flex-1"
      viewportProps={{ tabIndex: 0, role: 'region', 'aria-label': `${inspectorTitle} content`, className: 'overscroll-contain [&>div]:block!' }}>
      {inspectorBody()}
    </ScrollArea>
    {openInspector === 'generation' && (showGenerateAction || generationResult?.result?.asset_id) && <SidebarFooter className="shrink-0 border-t px-4 py-3">
      {showGenerateAction && <Button disabled={!generationInput || !generationPayload || busy || connection !== 'connected'}
        onClick={() => void generate()}>Generate 3D</Button>}
      {generationResult?.result?.asset_id && <Button variant="secondary" disabled={busy || isRunning} onClick={() => void openRecentJob(generationResult.id)}>View generated model</Button>}
    </SidebarFooter>}
  </>;
  const toolInspector = (
    <aside className="flex h-full min-h-0 min-w-0 flex-col" id="workbench-inspector" data-tool={openInspector}
      aria-labelledby="workbench-inspector-title">
      <SidebarHeader className="shrink-0 flex-row items-center gap-1 border-b px-4 py-2 font-heading text-sm font-medium">
        <h2 id="workbench-inspector-title" tabIndex={-1}>{inspectorTitle}</h2>
        {meshTool && <HelpTip label="About mesh tools">Tools create a new asset and never overwrite the original GLB.</HelpTip>}
      </SidebarHeader>
      {inspectorContent}
    </aside>
  );

  return (
    <>
      <Toaster theme={theme} closeButton visibleToasts={3} containerAriaLabel="Notifications"
        offset={{ right: 12, bottom: sidebar.narrow ? 80 : 12 }}
        mobileOffset={{ left: 12, right: 12, bottom: 80 }} />
      <SettingsPage api={api} section={settingsSection} backendId={settingsBackend} runtime={runtime}
        connection={connection} busy={busy || isRunning || installingBackend}
        jobs={recentJobs} operationLabel={studioOperationLabel} onJobDetails={showJobDetails}
        theme={theme} onThemeChange={setTheme}
        onVisit={openSettings} onClose={closeSettings}
        onOpenJobs={() => openJobs()} onActivityChange={setSettingsActivity}
        onReconnect={() => void connect(true)} onRefreshJobs={refreshRecentJobs} onRuntimeChange={setRuntime}
        onRuntimeAction={runRuntimeAction} onSetupEnvironment={() => void startRuntimeAction(() => api.setupModelEnvironment())} />
      <div className={`${settingsSection ? 'hidden' : 'flex'} h-dvh min-w-0 flex-col overflow-hidden bg-background`} onDragOver={(event) => {
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
        <div className="ml-auto flex flex-wrap items-center gap-1 sm:gap-2">
          <Button variant={sidebar.open && sidebar.panel === 'inventory' ? 'secondary' : 'ghost'} size="sm" ref={sidebar.inventoryTriggerRef}
            aria-expanded={sidebar.open && sidebar.panel === 'inventory'} aria-haspopup={sidebar.narrow ? 'dialog' : undefined}
            aria-controls={sidebar.open && sidebar.panel === 'inventory' ? (sidebar.narrow ? 'workbench-sidebar-drawer' : 'workbench-sidebar-dock') : undefined}
            onClick={() => sidebar.togglePanel('inventory')}><SquaresFourIcon aria-hidden="true" /> Inventory</Button>
          <Button variant={sidebar.open && sidebar.panel === 'jobs' ? 'secondary' : 'ghost'} size="sm" ref={sidebar.jobsTriggerRef}
            aria-expanded={sidebar.open && sidebar.panel === 'jobs'} aria-haspopup={sidebar.narrow ? 'dialog' : undefined}
            aria-controls={sidebar.open && sidebar.panel === 'jobs' ? (sidebar.narrow ? 'workbench-sidebar-drawer' : 'workbench-sidebar-dock') : undefined}
            onClick={() => { setFocusedJobId(null); sidebar.togglePanel('jobs'); }}><ListChecksIcon aria-hidden="true" /> Jobs</Button>
          <Button variant="ghost" size="sm" ref={settingsTrigger} onClick={() => openSettings('models')}><GearSixIcon data-icon="inline-start" aria-hidden="true" /> Settings</Button>
          {!settingsSection && <NotificationHistory />}
        </div>
      </header>

      <main className="relative flex min-h-0 min-w-0 flex-1 flex-col" data-connection={connection}>
          <h1 className="sr-only">MeshHarbor Studio</h1>
          <input ref={fileInputRef} className="sr-only" aria-label="Choose GLB files" tabIndex={-1} type="file" multiple accept=".glb,model/gltf-binary" onChange={(event) => { if (event.currentTarget.files) void importFiles(event.currentTarget.files); event.currentTarget.value = ''; }} />
          <input ref={inventoryInputRef} className="sr-only" aria-label="Choose image files" tabIndex={-1} type="file" multiple accept=".png,.jpg,.jpeg,image/png,image/jpeg" onChange={(event) => { if (event.currentTarget.files) void importFiles(event.currentTarget.files); event.currentTarget.value = ''; }} />
            <WorkbenchLayout state={settingsSection && sidebar.narrow ? { ...sidebar, open: false } : sidebar} inspector={toolInspector} tools={toolRail}
              content={sidebar.panel === 'jobs' ?
                <JobsPanel api={api} jobs={recentJobs} localActivities={localActivities} focusedJobId={focusedJobId}
                  connection={connection} loaded={jobFeedLoaded} resultBlocked={isRunning} retryBlocked={busy || isRunning}
                  operationLabel={studioOperationLabel} onRefresh={refreshJobHistory} onReconnect={() => void connect(true)}
                  onStop={requestStop} onDetails={showJobDetails}
                  onRetry={(target, trigger) => { retryTrigger.current = trigger; setRetryTarget(target); }}
                  onLoadOlder={before => api.jobs(JOB_PAGE_SIZE, { before })}
                  onViewResult={id => { if (sidebar.narrow) sidebar.setOpen(false); void openRecentJob(id); }} /> :
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
                {showLibraryFilters && <><form role="search" aria-label="Asset inventory" onSubmit={(event) => {
                  event.preventDefault();
                  applyAssetBrowserFilters({ ...assetFilters, query: assetSearchInput.trim() });
                }}>
                  <InputGroup>
                    <InputGroupInput
                      id="asset-filename"
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
                    <InputGroupAddon align="inline-end" className="py-0">
                      <InputGroupButton type="submit" aria-label="Search" title="Search" size="icon-xs"
                        disabled={connection !== 'connected'}><MagnifyingGlassIcon aria-hidden="true" /></InputGroupButton>
                    </InputGroupAddon>
                  </InputGroup>
                </form>
                <Disclosure className="min-w-0 text-muted-foreground" title={hasActiveAssetFilters ? 'Filters · Active' : 'Filters'}>
                  <div className="mt-2 grid gap-2">
                  {assetFilters.kind === 'mesh' && <Field orientation="horizontal">
                    <Checkbox id="show-all-versions" checked={assetFilters.allVersions === true} disabled={connection !== 'connected'}
                      onCheckedChange={checked => applyAssetBrowserFilters({ ...assetFilters, allVersions: checked === true })} />
                    <FieldLabel htmlFor="show-all-versions">Show all mesh versions</FieldLabel>
                  </Field>}
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
                      <NativeSelectOption value="original">{assetFilters.kind === 'mesh' && !assetFilters.allVersions ? 'Imported models' : 'Originals'}</NativeSelectOption>
                      <NativeSelectOption value="derived">{assetFilters.kind === 'mesh' && !assetFilters.allVersions ? 'Generated models' : 'Generated or derived assets'}</NativeSelectOption>
                    </NativeSelect>
                  </Field>
                  {hasActiveAssetFilters && (
                    <Button type="button" variant="ghost" size="sm" className="justify-self-start" onClick={() => applyAssetBrowserFilters({ ...EMPTY_ASSET_BROWSER_FILTERS, kind: assetFilters.kind })}>
                      Clear filters
                    </Button>
                  )}
                  </div>
                </Disclosure></>}
                <Disclosure className="min-w-0 text-muted-foreground" title="Find by ID">
                  <form className="mt-2 grid gap-2" onSubmit={event => { event.preventDefault(); void openStoredAsset(storedItemId.trim()); }}>
                    <Field>
                      <FieldLabel htmlFor="stored-item-id">Asset or artifact ID</FieldLabel>
                      <Input id="stored-item-id" value={storedItemId} onChange={event => setStoredItemId(event.currentTarget.value)} />
                    </Field>
                    <div className="flex flex-wrap gap-1">
                      <Button type="submit" variant="outline" size="sm" disabled={!storedItemId.trim() || busy || connection !== 'connected'}>Open asset</Button>
                      <Button type="button" variant="outline" size="sm" disabled={!storedItemId.trim() || downloading || connection !== 'connected'}
                        onClick={() => void downloadStoredArtifact(storedItemId.trim())}>Download artifact</Button>
                    </div>
                  </form>
                </Disclosure>
                </div>
                <div className="grid min-w-0 gap-3 px-3 py-3">
                {assetFilters.kind === 'mesh' && <PendingGenerationCards jobs={visiblePendingGenerationJobs} sources={referenceNames}
                  selectedId={pendingSelection?.id ?? null} onSelect={pending => selectPendingModel(pending)} />}
                {matchingAssets.length > 0 && <p className="text-xs text-muted-foreground" aria-live="polite">
                  {assetPageLoading ? 'Loading assets…' : `${matchingAssets.length} loaded ${hasActiveAssetFilters ? 'result' : 'asset'}${matchingAssets.length === 1 ? '' : 's'}`}
                </p>}
                {matchingAssets.length === 0
                  ? (assetFilters.kind !== 'mesh' || !visiblePendingGenerationJobs.length) && <Empty role="status"><EmptyHeader><EmptyDescription>
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

            <section className="@container flex min-h-0 min-w-0 flex-1 flex-col bg-muted dark:bg-neutral-800" aria-label={selectedPendingJob ? 'Pending model preview' : selectedAsset ? 'Selected asset' : 'Start a model'}>
              {selectedPendingJob ? <Empty className="m-auto min-h-0 w-full overflow-auto">
                <EmptyHeader>
                  <EmptyMedia variant="icon"><CubeIcon aria-hidden="true" /></EmptyMedia>
                  <EmptyTitle className="wrap-anywhere">{generationJobName(selectedPendingJob, referenceNames)}</EmptyTitle>
                  <EmptyDescription role="status">{jobStage(selectedPendingJob)}</EmptyDescription>
                </EmptyHeader>
                <EmptyContent><Button variant="outline" onClick={() => openJobs(selectedPendingJob.id)}>View in Jobs</Button></EmptyContent>
              </Empty> : !selectedAsset ? <div className="flex min-h-0 flex-1 overflow-y-auto">
                <div className="m-auto grid w-full max-w-4xl items-center gap-4 p-4 @min-[42rem]:grid-cols-[minmax(0,22rem)_minmax(0,24rem)] @min-[42rem]:gap-8 sm:p-6">
                <div className="order-2 grid min-w-0 gap-4 @min-[42rem]:order-first">
                  <h2 className="font-heading text-xl font-medium @min-[42rem]:text-3xl">Start with a model or an image</h2>
                  <div className="flex flex-wrap gap-2">
                    <Button onClick={() => {
                      if (assetFilters.kind !== 'mesh') applyAssetBrowserFilters(EMPTY_ASSET_BROWSER_FILTERS);
                      sidebar.showPanel('inventory');
                    }}><CubeIcon data-icon="inline-start" aria-hidden="true" /> Browse inventory</Button>
                    <Button variant="outline" onClick={() => setInspector('generation')}><ImageSquareIcon data-icon="inline-start" aria-hidden="true" /> Create from image</Button>
                  </div>
                  <p className="text-xs text-muted-foreground">Drop a GLB, PNG or JPEG here</p>
                </div>
                <MeshStudy />
                </div>
              </div> : <div className="flex min-h-0 min-w-0 flex-1 flex-col">
                <div className="flex shrink-0 flex-wrap items-center gap-2 border-b bg-card p-2 text-card-foreground">
                <div className="flex min-w-0 flex-1 items-center justify-between gap-2 max-sm:basis-full">
                  <div className="flex min-w-0 items-center gap-2 text-xs">{selectedAsset.kind === 'image' ? <FileImageIcon aria-hidden="true" /> : <CubeIcon aria-hidden="true" />} <span className="truncate font-medium" title={selectedAsset.name}>{selectedAsset.model?.name ?? selectedAsset.name}</span></div>
                  {selectedComparisonPair && (
                    <Tabs className="shrink-0" activationMode="manual"
                      value={activeView} onValueChange={value => {
                        if ((value === 'source' || value === 'result') && value !== activeView) void selectComparisonSide(value);
                      }}>
                      <TabsList aria-label="Preview version" aria-busy={comparisonLoadingSide !== null}>
                        {(['source', 'result'] as const).map(side => <TabsTrigger key={side} id={`preview-tab-${side}`} value={side}
                          aria-controls="preview-asset" disabled={comparisonLoadingSide !== null || busy || connection !== 'connected'}>
                          {side === 'source' ? 'Original' : 'Result'}
                        </TabsTrigger>)}
                      </TabsList>
                    </Tabs>
                  )}
                </div>
                {selectedAsset.kind === 'image' && <Button variant="secondary" size="sm" disabled={busy || isRunning} onClick={() => {
                  setGenerationSource(selectedAsset);
                  setPreferredViewMode('single');
                  setInspector('generation');
                }}><ImageSquareIcon data-icon="inline-start" aria-hidden="true" /> Use as reference</Button>}
                </div>
                <div id="preview-asset" role={selectedComparisonPair ? 'tabpanel' : undefined}
                  aria-labelledby={selectedComparisonPair ? `preview-tab-${activeView}` : undefined}
                  tabIndex={selectedComparisonPair ? 0 : undefined}
                  className="flex min-h-0 flex-1 flex-col">
                  <ModelPreview asset={previewAsset} comparison={previewComparison} loadModel={loadPreview} onError={reportPreviewError} theme={theme} />
                </div>
                {showVersionStats && <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-t bg-card px-3 py-2 text-xs tabular-nums text-muted-foreground [&_strong]:font-medium [&_strong]:text-foreground" role="group" aria-label="Original to result comparison">
                  {typeof beforeTriangles === 'number' && typeof afterTriangles === 'number' && <span>Triangles <strong>{beforeTriangles.toLocaleString()} → {afterTriangles.toLocaleString()}</strong></span>}
                  {comparisonSizes && <span>GLB size <strong>{comparisonSizes}</strong></span>}
                </div>}
                {selectedAsset && (
                  <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-t bg-card px-3 py-2 text-xs tabular-nums">
                    <div className="flex items-center gap-2 text-muted-foreground"><Badge variant="outline">{selectedAsset.kind === 'mesh' ? 'GLB' : selectedAsset.mime_type === 'image/png' ? 'PNG' : 'JPG'}</Badge><div><span>{formatSize(selectedAsset.byte_size)} · {selectedAsset.kind === 'mesh' ? `${Number(selectedAsset.metadata.mesh_count ?? 0)} mesh${Number(selectedAsset.metadata.mesh_count ?? 0) === 1 ? '' : 'es'}` : 'Reference image'}</span></div></div>
                    <Separator orientation="vertical" className="ml-auto hidden h-4 sm:block" />
                    {selectedAsset.kind === 'mesh' ? <><div className="grid gap-1 [&>span]:text-muted-foreground [&>strong]:font-medium"><span>Triangles</span><strong>{triangleCount.toLocaleString()}</strong></div><div className="grid gap-1 [&>span]:text-muted-foreground [&>strong]:font-medium"><span>Materials</span><strong>{Number(selectedAsset.metadata.material_count ?? 0)}</strong></div></> : <div className="grid gap-1 [&>span]:text-muted-foreground [&>strong]:font-medium"><span>Input</span><strong>{selectedAsset.mime_type === 'image/png' ? 'PNG' : 'JPEG'}</strong></div>}
                    <div className="grid gap-1 [&>span]:text-muted-foreground [&>strong]:font-medium">
                      <span>{selectedAsset.model_version ? `Version ${selectedAsset.model_version}` : 'Version'}</span><strong>{selectedAsset.model
                        ? selectedAsset.id === selectedAsset.model.current_asset_id ? 'Current' : 'Historical'
                        : selectedAsset.source_asset_id ? 'Result' : 'Original'}</strong>
                    </div>
                  </div>
                )}
              {reportCard && (
                <Disclosure className="min-w-0 shrink-0 border-t bg-card p-3 text-xs" label={`${reportCard.title} result`} title="Operation report">
                  <ScrollArea type="auto" viewportProps={{ tabIndex: 0, role: 'region', 'aria-label': 'Report details', className: 'max-h-48 overscroll-contain pr-2 [&>div]:block!' }}>
                  <div className="mt-3 font-medium"><strong>{reportCard.title} complete</strong></div>
                  {!reportCard.reportAvailable && <p className="mt-2 text-muted-foreground">The operation completed, but its report payload is missing or unreadable.</p>}
                  <ItemGroup className="mt-3 grid grid-cols-2 sm:grid-cols-3" aria-label="Operation metrics">
                    {reportCard.metrics.map((metric) => (
                      <Item key={metric.label} role="listitem" variant="outline" size="xs" className="min-w-0">
                        <ItemContent className="min-w-0 wrap-anywhere">
                          <ItemDescription className="line-clamp-none">{metric.label}</ItemDescription>
                          <ItemTitle className="line-clamp-none max-w-full">{metric.value}</ItemTitle>
                        </ItemContent>
                      </Item>
                    ))}
                  </ItemGroup>
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
                  </ScrollArea>
                </Disclosure>
              )}
                {hasSelectedMesh && <div className="flex shrink-0 flex-wrap justify-end gap-2 border-t bg-card px-3 py-2" role="group" aria-label="File actions">
                  {selectedAsset.model && <div className="mr-auto">
                    <ModelVersionHistory key={selectedAsset.model.id} api={api} model={selectedAsset.model} viewingId={selectedAsset.id}
                      disabled={connection !== 'connected' || busy || comparisonLoadingSide !== null}
                      operationLabel={studioOperationLabel} onView={showModelVersion} />
                  </div>}
                  {quadArtifact && <Button variant="secondary" size="sm" title="Editable quad topology without textures or UVs" disabled={downloading || comparisonLoadingSide !== null || connection !== 'connected'} onClick={() => void downloadResult(quadArtifact)}>Quad OBJ</Button>}
                  <BlenderExport api={api} asset={selectedAsset} disabled={comparisonLoadingSide !== null || connection !== 'connected'} />
                  <Button variant={exportIsPrimary ? 'default' : 'secondary'} size="sm" data-review-action={exportIsPrimary || undefined} title="Export GLB" aria-label="Export selected GLB" disabled={downloading || comparisonLoadingSide !== null || connection !== 'connected'} onClick={() => void downloadResult()}><DownloadSimpleIcon aria-hidden="true" /> Export GLB</Button>
                </div>}
                {selectedAsset.kind === 'image' && <div className="flex shrink-0 justify-end border-t bg-card px-3 py-2">
                  <Button variant="secondary" size="sm" disabled={downloading || connection !== 'connected'} onClick={() => void downloadResult()}>
                    <DownloadSimpleIcon aria-hidden="true" /> Export image
                  </Button>
                </div>}
              </div>}

            </section>

            </WorkbenchLayout>
      </main>
      {dragging && <div className="pointer-events-none fixed inset-0 z-30 grid place-items-center bg-background/80 p-4">
        <Empty className="w-fit border bg-card">
          <EmptyHeader>
            <EmptyMedia variant="icon"><ArrowDownIcon className="text-primary" aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>Drop a GLB or source image here</EmptyTitle>
            <EmptyDescription>Import starts locally on your computer</EmptyDescription>
          </EmptyHeader>
        </Empty>
      </div>}

      </div>
      {currentDetails && <JobDetails key={currentDetails.id} api={api} job={currentDetails} title={studioOperationLabel(currentDetails.operation)}
        onClose={() => setDetailsTarget(null)} returnFocus={() => {
          const trigger = detailsTrigger.current;
          if (trigger?.isConnected && trigger.getClientRects().length) trigger.focus();
          else focusPanelHeading();
        }} />}
      <AlertDialog open={stopTarget !== null} onOpenChange={open => { if (!open && !stoppingJob) setStopTarget(null); }}>
        <AlertDialogContent onEscapeKeyDown={event => { if (stoppingJob) event.preventDefault(); }}
          onCloseAutoFocus={event => {
            event.preventDefault();
            const trigger = stopTrigger.current;
            if (trigger?.isConnected && !trigger.disabled && trigger.getClientRects().length) trigger.focus();
            else focusPanelHeading();
          }}>
          <AlertDialogHeader>
            <AlertDialogTitle>{stopTarget?.state === 'queued' ? 'Remove this queued job?' : 'Stop this job?'}</AlertDialogTitle>
            <AlertDialogDescription>
              {stopTarget?.state === 'queued' ? 'Removes the job before it starts.'
                : stopTarget?.operation.startsWith('runtime.') ? 'Downloaded files are kept for a retry.'
                  : isGenerationOperation(stopTarget?.operation ?? '') ? 'Stops the worker without saving a result.'
                    : 'Stops at the next safe checkpoint without saving a result.'}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={stoppingJob}>Keep job</AlertDialogCancel>
            <AlertDialogAction variant="destructive" disabled={stoppingJob} onClick={event => {
              event.preventDefault();
              void stopJob();
            }}>{stoppingJob ? 'Requesting stop…' : stopTarget?.state === 'queued' ? 'Remove from queue' : 'Stop job'}</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog open={retryTarget !== null} onOpenChange={open => { if (!open && !busy) setRetryTarget(null); }}>
        <AlertDialogContent onCloseAutoFocus={event => {
          event.preventDefault();
          const trigger = retryTrigger.current;
          if (trigger?.isConnected && !trigger.disabled) trigger.focus();
          else focusPanelHeading();
        }}>
          <AlertDialogHeader>
            <AlertDialogTitle>Retry this job?</AlertDialogTitle>
            <AlertDialogDescription>
              With the same input and settings, this starts a new attempt and keeps the previous record.
              {retryTarget?.operation.startsWith('runtime.') && ' The attempt may download large files and reuses available cached data.'}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
            <AlertDialogAction disabled={busy || isRunning || connection !== 'connected'} onClick={event => {
              event.preventDefault();
              void retryJob();
            }}>{busy ? 'Starting…' : 'Retry job'}</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

function studioOperationLabel(operation: string): string {
  switch (operation) {
    case 'mesh.simplify': return 'Optimize mesh';
    case 'mesh.separate_parts': return 'Connected parts (retired)';
    case 'model.generate':
    case 'image.generate': return 'Generate model';
    case 'uv.unwrap': return 'Unwrap UVs';
    case 'runtime.environment.setup': return 'Set up environment';
    case 'runtime.pixal3d.setup': return 'Set up MLX';
    case 'runtime.pixal3d.install_models': return 'Download MLX models';
    case 'runtime.pixal3d.cuda_setup': return 'Set up CUDA';
    case 'runtime.pixal3d.cuda_install_models': return 'Download CUDA models';
    case 'runtime.pixal3d.cuda_install_multiview': return 'Install multiview weights';
    case 'runtime.pixal3d.convert_naf': return 'Convert NAF checkpoint';
    case 'runtime.models.install': return 'Install model';
    case 'runtime.geosam2.install_backend': return 'Install GeoSAM2 backend';
    case 'runtime.geosam2.download_weights': return 'Download GeoSAM2 weights';
    default: return meshOperationOptions.find(tool => tool.id === operation)?.title ?? operation;
  }
}
