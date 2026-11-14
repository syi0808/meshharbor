import type { ApiErrorBody, Artifact, Asset, AssetKind, AssetPage, Job, MeshOperationId, Operation, RuntimeInfo } from './types';

export const MAX_ASSET_IMPORT_FILES = 10;
export const MAX_IMAGE_IMPORT_BYTES = 16 * 1024 * 1024;
export const MAX_GLB_IMPORT_BYTES = 512 * 1024 * 1024;

export type AssetImportKind = 'mesh' | 'image';

export interface AssetImportFailure {
  index: number;
  fileName: string;
  message: string;
}

export interface AssetImportProgress {
  current: number;
  total: number;
  fileName: string;
}

export interface AssetImportBatchResult<TAsset> {
  assets: TAsset[];
  failures: AssetImportFailure[];
  skippedCount: number;
  attemptedCount: number;
}

export interface AssetImportUiSnapshot {
  selectedId: string | null;
  filters: { query: string; kind: string; origin: string };
  searchInput: string;
  browserInteractionRevision: number;
  selectionInteractionRevision: number;
}

/** Keep user changes made while sequential uploads are in flight. */
export function planAssetImportUiCompletion(
  started: AssetImportUiSnapshot,
  current: AssetImportUiSnapshot,
): { resetFilters: boolean; selectFirstImportedAsset: boolean } {
  const sameFilters = started.filters.query === current.filters.query
    && started.filters.kind === current.filters.kind
    && started.filters.origin === current.filters.origin;
  return {
    resetFilters: sameFilters
      && started.searchInput === current.searchInput
      && started.browserInteractionRevision === current.browserInteractionRevision,
    selectFirstImportedAsset: started.selectedId === current.selectedId
      && started.selectionInteractionRevision === current.selectionInteractionRevision,
  };
}

/**
 * Upload a bounded selection in order. Invalid files are reported without a request;
 * each valid file is attempted once, and one failure never prevents later files.
 */
export async function importAssetBatch<TFile extends { name: string; size: number }, TAsset>(
  files: ArrayLike<TFile>,
  upload: (file: TFile, kind: AssetImportKind) => Promise<TAsset>,
  onProgress?: (progress: AssetImportProgress) => void,
): Promise<AssetImportBatchResult<TAsset>> {
  const totalFiles = Number.isFinite(files.length) ? Math.max(0, Math.floor(files.length)) : 0;
  const boundedCount = Math.min(totalFiles, MAX_ASSET_IMPORT_FILES);
  const candidates: Array<{ file: TFile; kind: AssetImportKind; index: number }> = [];
  const failures: AssetImportFailure[] = [];

  for (let index = 0; index < boundedCount; index += 1) {
    const file = files[index];
    if (!file) continue;
    const name = file.name.toLowerCase();
    const kind: AssetImportKind | null = name.endsWith('.glb')
      ? 'mesh'
      : /\.(png|jpe?g)$/.test(name) ? 'image' : null;
    if (!kind) {
      failures.push({ index, fileName: file.name, message: 'Unsupported file type. Choose a .glb, .png, .jpg, or .jpeg file.' });
      continue;
    }
    if (!Number.isFinite(file.size) || file.size < 0) {
      failures.push({ index, fileName: file.name, message: 'Could not determine the file size.' });
      continue;
    }
    if (file.size === 0) {
      failures.push({ index, fileName: file.name, message: 'The file is empty.' });
      continue;
    }
    const sizeLimit = kind === 'image' ? MAX_IMAGE_IMPORT_BYTES : MAX_GLB_IMPORT_BYTES;
    if (file.size > sizeLimit) {
      const sizeLimitMiB = sizeLimit / (1024 * 1024);
      failures.push({
        index,
        fileName: file.name,
        message: `File exceeds the ${sizeLimitMiB} MiB ${kind === 'image' ? 'image' : 'GLB'} limit.`,
      });
      continue;
    }
    candidates.push({ file, kind, index });
  }

  const assets: TAsset[] = [];
  let attemptedCount = 0;
  for (const candidate of candidates) {
    attemptedCount += 1;
    onProgress?.({
      current: attemptedCount,
      total: candidates.length,
      fileName: candidate.file.name,
    });
    try {
      assets.push(await upload(candidate.file, candidate.kind));
    } catch (cause) {
      const message = cause instanceof Error ? cause.message.trim() : '';
      failures.push({
        index: candidate.index,
        fileName: candidate.file.name,
        message: message || 'Upload failed.',
      });
    }
  }

  failures.sort((left, right) => left.index - right.index);
  return {
    assets,
    failures,
    skippedCount: Math.max(0, totalFiles - boundedCount),
    attemptedCount,
  };
}

export interface AssetPageFilters {
  query?: string;
  kind?: AssetKind;
  derived?: boolean;
}

export function quadTopologyArtifact(asset: Asset | null): Artifact | undefined {
  return asset?.artifacts.find((entry) => entry.kind === 'quad_topology' || entry.kind === 'quad_obj');
}

export interface AssetComparisonPair {
  inputAssetId: string;
  resultAssetId: string;
}

export type AssetComparisonSide = 'source' | 'result';

export function assetComparisonPairFromJob(
  job: Job,
  resultAsset: Asset | null,
): AssetComparisonPair | null {
  const inputAssetId = job.input_asset_id;
  const resultAssetId = job.result?.asset_id;
  if (!inputAssetId || !resultAssetId || !resultAsset || resultAsset.id !== resultAssetId
    || resultAssetId === inputAssetId
    || resultAsset.source_asset_id !== inputAssetId) {
    return null;
  }
  return { inputAssetId, resultAssetId };
}

export function assetComparisonPairForSelection(
  assets: readonly Asset[],
  selectedAssetId: string | null,
  pinnedPair: AssetComparisonPair | null,
  activeJobInputAssetId: string | null = null,
): AssetComparisonPair | null {
  if (!selectedAssetId || selectedAssetId === activeJobInputAssetId) return null;
  if (pinnedPair
    && (pinnedPair.inputAssetId === selectedAssetId || pinnedPair.resultAssetId === selectedAssetId)) {
    return pinnedPair;
  }

  const selected = assets.find((asset) => asset.id === selectedAssetId);
  if (!selected) return null;
  if (selected.source_asset_id) {
    return { inputAssetId: selected.source_asset_id, resultAssetId: selected.id };
  }

  const result = assets.find((asset) => asset.id !== selected.id && asset.source_asset_id === selected.id);
  return result ? { inputAssetId: selected.id, resultAssetId: result.id } : null;
}

export function assetComparisonTarget(
  pair: AssetComparisonPair,
  side: AssetComparisonSide,
): string {
  return side === 'source' ? pair.inputAssetId : pair.resultAssetId;
}

export function shouldAutoSelectComparisonResult(
  pair: AssetComparisonPair | null,
  selectedAssetId: string | null,
): boolean {
  return pair !== null && selectedAssetId === pair.inputAssetId;
}

export async function resolveComparisonAsset(
  pair: AssetComparisonPair,
  side: AssetComparisonSide,
  assets: readonly Asset[],
  loadAsset: (assetId: string) => Promise<Asset>,
): Promise<Asset> {
  const assetId = assetComparisonTarget(pair, side);
  const asset = assets.find((candidate) => candidate.id === assetId) ?? await loadAsset(assetId);
  if (asset.id !== assetId) {
    throw new Error('The comparison asset response did not match the requested asset.');
  }
  if (side === 'result' && asset.source_asset_id !== pair.inputAssetId) {
    throw new Error('The saved result does not match this comparison source.');
  }
  return asset;
}

export function cancelJobNotice(job: Job): string {
  if (job.state === 'cancelled') return 'The queued job was cancelled.';
  if (job.state === 'queued') return 'This job is still queued. Try cancelling it again.';
  if (job.state === 'running' || job.state === 'validating' || job.state === 'cancelling') {
    return 'The job started before cancellation took effect. Running work cannot be interrupted safely.';
  }
  return 'The job finished before cancellation took effect.';
}

export function isQueuedJobCancellable(job: Job | null): job is Job & { state: 'queued' } {
  return job?.state === 'queued';
}

export function isActiveJob(
  job: Job | null,
): job is Job & { state: 'queued' | 'running' | 'validating' | 'cancelling' } {
  return job !== null && ['queued', 'running', 'validating', 'cancelling'].includes(job.state);
}

export function canInspectJobHistoryRecord(
  trackedJob: Job | null,
  historyJob: Job,
): boolean {
  return !isActiveJob(trackedJob) || trackedJob.id === historyJob.id;
}

export function preferTrackedTerminalJob(trackedJob: Job | null, historyJob: Job): Job {
  return trackedJob?.id === historyJob.id && !isActiveJob(trackedJob)
    ? trackedJob
    : historyJob;
}

export function shouldDeferTrackedJobCompletion(trackedJob: Job | null, historyJob: Job): boolean {
  return isActiveJob(trackedJob)
    && trackedJob.id === historyJob.id
    && !isActiveJob(historyJob);
}

export class LatestRequestGuard {
  private sequence = 0;

  begin(): number {
    this.sequence += 1;
    return this.sequence;
  }

  isCurrent(requestId: number): boolean {
    return requestId === this.sequence;
  }

  invalidate(): void {
    this.sequence += 1;
  }
}

export function jobPollRetryDelay(failures: number): number {
  const failureCount = Number.isFinite(failures) ? Math.max(1, Math.floor(failures)) : 1;
  const exponent = Math.min(5, failureCount - 1);
  return Math.min(10_000, 500 * 2 ** exponent);
}

export type JobReportListState = 'available' | 'empty' | 'partial' | 'unavailable';

export interface JobReportMetric {
  label: string;
  value: string;
}

export interface JobReportList {
  label: string;
  state: JobReportListState;
  values: string[];
  omittedCount: number;
}

export interface JobReportCardModel {
  title: string;
  jobId: string;
  inputAssetId: string | null;
  resultAssetId: string | null;
  reportAvailable: boolean;
  metrics: JobReportMetric[];
  lists: JobReportList[];
  warnings: string[];
  warningsState: 'available' | 'empty' | 'unavailable';
  warningsMessage: string;
  warningFormatIssue: boolean;
}

type JobReportFormatter = 'count' | 'bytes' | 'milliseconds' | 'decimal' | 'percent' | 'text' | 'boolean';

interface JobReportField {
  key: string;
  label: string;
  formatter: JobReportFormatter;
}

interface JobReportListField {
  key: string;
  label: string;
  formatter: 'text-list' | 'count-list';
}

interface JobReportSchema {
  title: string;
  metrics: JobReportField[];
  lists?: JobReportListField[];
}

const MAX_REPORT_LIST_ITEMS = 256;

const JOB_REPORT_SCHEMAS: Record<string, JobReportSchema> = {
  'image.generate': {
    title: 'Image-to-3D generation',
    metrics: [
      { key: 'backend_id', label: 'Backend', formatter: 'text' },
      { key: 'seed', label: 'Seed', formatter: 'count' },
      { key: 'resolution', label: 'Resolution', formatter: 'count' },
      { key: 'texture_size', label: 'Texture size', formatter: 'count' },
      { key: 'target_faces', label: 'Target faces', formatter: 'count' },
      { key: 'elapsed_ms', label: 'Elapsed time', formatter: 'milliseconds' },
    ],
  },
  'mesh.simplify': {
    title: 'Simplification',
    metrics: [
      { key: 'triangles_before', label: 'Triangles before', formatter: 'count' },
      { key: 'triangles_after', label: 'Triangles after', formatter: 'count' },
      { key: 'target_triangles', label: 'Target triangles', formatter: 'count' },
      { key: 'geometric_error', label: 'Geometric error', formatter: 'decimal' },
      { key: 'optimized_primitives', label: 'Optimized primitives', formatter: 'count' },
    ],
    lists: [{ key: 'preserved', label: 'Preserved data', formatter: 'text-list' }],
  },
  'mesh.cleanup': {
    title: 'Mesh cleanup',
    metrics: [
      { key: 'triangles_before', label: 'Triangles before', formatter: 'count' },
      { key: 'triangles_after', label: 'Triangles after', formatter: 'count' },
      { key: 'repeated_index_triangles_removed', label: 'Repeated-index triangles removed', formatter: 'count' },
      { key: 'zero_area_triangles_removed', label: 'Zero-area triangles removed', formatter: 'count' },
      { key: 'duplicate_triangles_removed', label: 'Duplicate triangles removed', formatter: 'count' },
      { key: 'modified_primitives', label: 'Modified primitives', formatter: 'count' },
    ],
    lists: [{ key: 'preserved', label: 'Preserved data', formatter: 'text-list' }],
  },
  'render.optimize': {
    title: 'Render optimization',
    metrics: [
      { key: 'triangles', label: 'Triangles', formatter: 'count' },
      { key: 'processed_primitives', label: 'Processed primitives', formatter: 'count' },
      { key: 'reordered_primitives', label: 'Reordered primitives', formatter: 'count' },
      { key: 'skipped_translucent_primitives', label: 'Translucent primitives skipped', formatter: 'count' },
      { key: 'vertex_cache_size', label: 'Vertex cache size', formatter: 'count' },
      { key: 'vertex_cache_misses_before', label: 'Cache misses before', formatter: 'count' },
      { key: 'vertex_cache_misses_after', label: 'Cache misses after', formatter: 'count' },
      { key: 'overdraw_threshold', label: 'Overdraw threshold', formatter: 'decimal' },
    ],
    lists: [{ key: 'preserved', label: 'Preserved data', formatter: 'text-list' }],
  },
  'mesh.compress': {
    title: 'Mesh compression',
    metrics: [
      { key: 'input_bytes', label: 'Input size', formatter: 'bytes' },
      { key: 'output_bytes', label: 'Output size', formatter: 'bytes' },
      { key: 'bytes_saved', label: 'Bytes saved', formatter: 'bytes' },
      { key: 'compressed_buffer_views', label: 'Compressed buffer views', formatter: 'count' },
      { key: 'uncompressed_buffer_views', label: 'Uncompressed buffer views', formatter: 'count' },
      { key: 'extension', label: 'Required extension', formatter: 'text' },
      { key: 'extension_required', label: 'Extension required', formatter: 'boolean' },
    ],
  },
  'mesh.separate_parts': {
    title: 'Connected parts',
    metrics: [
      { key: 'triangles_before', label: 'Triangles before', formatter: 'count' },
      { key: 'connected_parts', label: 'Connected parts', formatter: 'count' },
      { key: 'method', label: 'Method', formatter: 'text' },
    ],
    lists: [{ key: 'triangles_per_part', label: 'Triangles per part', formatter: 'count-list' }],
  },
  'uv.unwrap': {
    title: 'UV unwrap',
    metrics: [
      { key: 'source_triangles', label: 'Source triangles', formatter: 'count' },
      { key: 'output_triangles', label: 'Output triangles', formatter: 'count' },
      { key: 'source_vertices', label: 'Source vertices', formatter: 'count' },
      { key: 'output_vertices', label: 'Output vertices', formatter: 'count' },
      { key: 'atlas_width', label: 'Atlas width', formatter: 'count' },
      { key: 'atlas_height', label: 'Atlas height', formatter: 'count' },
      { key: 'chart_count', label: 'Charts', formatter: 'count' },
      { key: 'covered_texels', label: 'Covered texels', formatter: 'count' },
      { key: 'gutter_texels', label: 'Gutter texels', formatter: 'count' },
    ],
    lists: [
      { key: 'preserved_attributes', label: 'Preserved attributes', formatter: 'text-list' },
      { key: 'rebaked_maps', label: 'Rebaked maps', formatter: 'text-list' },
    ],
  },
  'mesh.retopology': {
    title: 'Quad retopology',
    metrics: [
      { key: 'input_vertices', label: 'Input vertices', formatter: 'count' },
      { key: 'input_triangles', label: 'Input triangles', formatter: 'count' },
      { key: 'requested_quads', label: 'Requested quads', formatter: 'count' },
      { key: 'output_vertices', label: 'Output vertices', formatter: 'count' },
      { key: 'output_quads', label: 'Output quads', formatter: 'count' },
      { key: 'preview_triangles', label: 'Preview triangles', formatter: 'count' },
      { key: 'algorithm', label: 'Algorithm', formatter: 'text' },
    ],
  },
};

function reportObject(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function formatReportField(value: unknown, formatter: JobReportFormatter): string {
  if (formatter === 'text') {
    return typeof value === 'string' && value.trim() ? value : 'Not reported';
  }
  if (formatter === 'boolean') {
    return typeof value === 'boolean' ? (value ? 'Required' : 'Not required') : 'Not reported';
  }
  if (formatter === 'decimal') {
    return typeof value === 'number'
      && Number.isFinite(value)
      && value >= 0
      && value <= Number.MAX_SAFE_INTEGER
      ? value.toLocaleString(undefined, { maximumSignificantDigits: 6 })
      : 'Not reported';
  }
  if (formatter === 'percent') {
    return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 1
      ? `${(value * 100).toFixed(1)}%`
      : 'Not reported';
  }
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) return 'Not reported';
  const formatted = value.toLocaleString();
  if (formatter === 'bytes') return `${formatted} B`;
  return formatter === 'milliseconds' ? `${formatted} ms` : formatted;
}

function reportList(
  object: Record<string, unknown> | null,
  field: JobReportListField,
  labelPrefix = '',
): JobReportList {
  const raw = object?.[field.key];
  if (!Array.isArray(raw)) {
    return { label: `${labelPrefix}${field.label}`, state: 'unavailable', values: [], omittedCount: 0 };
  }
  if (raw.length === 0) {
    return { label: `${labelPrefix}${field.label}`, state: 'empty', values: [], omittedCount: 0 };
  }

  let invalidEntry = false;
  const displayed = raw.slice(0, MAX_REPORT_LIST_ITEMS);
  const omittedCount = raw.length - displayed.length;
  const values = displayed.map((entry, index) => {
    if (field.formatter === 'count-list') {
      const value = formatReportField(entry, 'count');
      if (value === 'Not reported') invalidEntry = true;
      return `Part ${index + 1}: ${value}`;
    }
    if (typeof entry !== 'string' || !entry.trim()) {
      invalidEntry = true;
      return null;
    }
    return entry;
  }).filter((entry): entry is string => entry !== null);

  return {
    label: `${labelPrefix}${field.label}`,
    state: invalidEntry || omittedCount > 0 ? 'partial' : 'available',
    values,
    omittedCount,
  };
}

interface ReportWarnings {
  available: boolean;
  formatIssue: boolean;
  values: string[];
}

function readReportWarnings(object: Record<string, unknown> | null, prefix = ''): ReportWarnings {
  const raw = object?.warnings;
  if (!Array.isArray(raw)) return { available: false, formatIssue: false, values: [] };
  let formatIssue = false;
  const values: string[] = [];
  for (const entry of raw) {
    if (typeof entry !== 'string') {
      formatIssue = true;
      continue;
    }
    values.push(`${prefix}${entry}`);
  }
  return { available: true, formatIssue, values };
}

/** Turn known operation reports into a curated, text-only card model. */
export function jobResultCardModel(job: Job | null | undefined): JobReportCardModel | null {
  if (!job || job.state !== 'succeeded') return null;
  const schema = JOB_REPORT_SCHEMAS[job.operation];
  if (!schema) return null;

  const resultObject = reportObject(job.result);
  const report = job.operation === 'image.generate' ? resultObject : reportObject(resultObject?.report);
  const metrics = schema.metrics.map((field) => ({
    label: field.label,
    value: formatReportField(report?.[field.key], field.formatter),
  }));
  const lists = (schema.lists ?? []).map((field) => reportList(report, field));
  let warnings = readReportWarnings(report);
  let warningsState: JobReportCardModel['warningsState'] = warnings.available ? 'empty' : 'unavailable';
  if (warnings.values.length > 0) warningsState = 'available';
  let warningFormatIssue = warnings.formatIssue;
  const warningValues = [...warnings.values];

  if (job.operation === 'mesh.retopology') {
    const projection = reportObject(report?.appearance_projection);
    if (projection) {
      const projectedMetrics: JobReportField[] = [
        { key: 'source_triangles', label: 'Source triangles', formatter: 'count' },
        { key: 'target_triangles', label: 'Target triangles', formatter: 'count' },
        { key: 'output_triangles', label: 'Output triangles', formatter: 'count' },
        { key: 'output_vertices', label: 'Output vertices', formatter: 'count' },
        { key: 'atlas_width', label: 'Atlas width', formatter: 'count' },
        { key: 'atlas_height', label: 'Atlas height', formatter: 'count' },
        { key: 'chart_count', label: 'Charts', formatter: 'count' },
        { key: 'covered_texels', label: 'Covered texels', formatter: 'count' },
        { key: 'gutter_texels', label: 'Gutter texels', formatter: 'count' },
        { key: 'projection_attempted_texels', label: 'Projection texels attempted', formatter: 'count' },
        { key: 'projection_covered_texels', label: 'Projection texels covered', formatter: 'count' },
        { key: 'projection_coverage', label: 'Projection coverage', formatter: 'percent' },
        { key: 'ambiguous_texels', label: 'Ambiguous texels', formatter: 'count' },
        { key: 'maximum_projection_distance', label: 'Maximum projection distance', formatter: 'decimal' },
        { key: 'projection_distance_limit', label: 'Projection distance limit', formatter: 'decimal' },
      ];
      metrics.push(...projectedMetrics.map((field) => ({
        label: `Appearance · ${field.label}`,
        value: formatReportField(projection[field.key], field.formatter),
      })));
      lists.push(reportList(projection, { key: 'rebaked_maps', label: 'Rebaked maps', formatter: 'text-list' }, 'Appearance · '));

      const projectionWarnings = readReportWarnings(projection, 'Appearance projection: ');
      warningFormatIssue ||= !projectionWarnings.available || projectionWarnings.formatIssue;
      if (projectionWarnings.values.length > 0) {
        warningValues.push(...projectionWarnings.values);
        warningsState = 'available';
      }
    }
  }
  if (warningFormatIssue && warningValues.length === 0) warningsState = 'unavailable';

  return {
    title: schema.title,
    jobId: typeof job.id === 'string' && job.id ? job.id : 'Not reported',
    inputAssetId: typeof job.input_asset_id === 'string' && job.input_asset_id ? job.input_asset_id : null,
    resultAssetId: typeof job.result?.asset_id === 'string' && job.result.asset_id ? job.result.asset_id : null,
    reportAvailable: report !== null,
    metrics,
    lists,
    warnings: warningValues,
    warningsState,
    warningsMessage: job.operation === 'image.generate'
      ? 'Image generation does not include a warning list in its saved result.'
      : warningFormatIssue
        ? 'Warning details are missing or malformed in this saved result.'
        : 'Warning list unavailable in this report.',
    warningFormatIssue,
  };
}

export function jobReportMatchesAsset(job: Job | null, assetId: string | null): boolean {
  return job?.state === 'succeeded'
    && typeof assetId === 'string'
    && assetId.length > 0
    && (assetId === job.input_asset_id || assetId === job.result?.asset_id);
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

  assetPage(limit = 50, cursor: string | null = null, filters: AssetPageFilters = {}): Promise<AssetPage> {
    const boundedLimit = Number.isFinite(limit) ? Math.min(100, Math.max(1, Math.trunc(limit))) : 50;
    const params = new URLSearchParams({ limit: String(boundedLimit) });
    if (cursor) params.set('cursor', cursor);
    const query = filters.query?.trim();
    if (query) params.set('q', query);
    if (filters.kind) params.set('kind', filters.kind);
    if (filters.derived !== undefined) params.set('derived', String(filters.derived));
    return this.request(`/api/v1/assets/page?${params.toString()}`);
  }

  jobs(limit = 20): Promise<Job[]> {
    const boundedLimit = Number.isFinite(limit) ? Math.min(100, Math.max(1, Math.trunc(limit))) : 20;
    return this.request(`/api/v1/jobs?limit=${boundedLimit}`);
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

