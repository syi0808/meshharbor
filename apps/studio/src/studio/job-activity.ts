import { isActiveJob, isGenerationOperation } from './api';
import type { Job } from './types';

export const JOB_PAGE_SIZE = 20;

export function jobStateLabel(state: Job['state']): string {
  return state === 'succeeded' ? 'Complete' : state === 'cancelling' ? 'Stopping'
    : state === 'cancelled' ? 'Stopped' : state[0].toUpperCase() + state.slice(1);
}

export function generationJobName(job: Job, sources: ReadonlyMap<string, string>): string {
  return job.input_asset_name || (job.input_asset_id && sources.get(job.input_asset_id)) || 'New model';
}

const stageLabels: Partial<Record<string, string>> = {
  check_runtime: 'Checking runtime availability',
  prepare_runtime: 'Preparing local runtime',
  prepare_source: 'Checking out pinned source',
  install_dependencies: 'Installing model dependencies',
  download_models: 'Downloading pinned model files',
  convert_models: 'Converting model weights',
  verify_models: 'Verifying model files',
  prepare_uv: 'Preparing uv',
  prepare_python: 'Checking existing Python environment',
  install_python: 'Installing managed Python',
  install_account_tools: 'Installing shared Hugging Face tools',
  check_huggingface_login: 'Verifying Hugging Face sign-in',
  create_environment: 'Creating Python environment',
  resolve_dependencies: 'Resolving dependency lock',
  sync_dependencies: 'Synchronizing dependencies',
  verify_environment: 'Verifying Python environment',
  check_model_access: 'Checking gated model access',
  download_weights: 'Reusing cache and downloading weights',
  verify_weights: 'Verifying model files',
  verify_checkpoint: 'Verifying checkpoint checksum',
  convert_checkpoint: 'Converting checkpoint',
  load_mesh: 'Loading source mesh',
  process_mesh: 'Processing mesh',
  voxelize_mesh: 'Rebuilding surface',
  validate_voxel_mesh: 'Checking rebuilt topology',
  prepare_references: 'Preparing reference images',
  initialize_model: 'Initializing model',
  preprocess_camera: 'Preparing images and cameras',
  dino_features: 'Extracting image features',
  sparse_structure: 'Building coarse 3D structure',
  shape_lr: 'Generating coarse geometry',
  coordinate_subdivision: 'Subdividing geometry',
  hr_conditioning: 'Preparing high-resolution geometry',
  shape_hr: 'Refining geometry',
  shape_decode: 'Decoding geometry',
  texture_conditioning: 'Preparing textures',
  texture_flow: 'Generating textures',
  texture_decode: 'Decoding textures',
  text_conditioning: 'Encoding text prompt',
  generate_latents: 'Generating shape latents',
  generate_mesh: 'Generating mesh',
  mesh_export: 'Exporting geometry GLB',
  pbr_export: 'Baking textures and exporting GLB',
  validate_output: 'Validating generated GLB',
  save_model: 'Saving model to inventory',
};

export function jobStepLabel(stage: string): string {
  return stageLabels[stage] ?? stage.replaceAll('_', ' ');
}

export function jobStage(job: Job): string {
  switch (job.state) {
    case 'queued': return 'Waiting for an engine slot';
    case 'validating': return 'Finalizing result';
    case 'cancelling': return isGenerationOperation(job.operation) ? 'Stopping generation worker'
      : job.operation.startsWith('runtime.') ? 'Stopping runtime worker' : 'Stopping at a safe checkpoint';
    case 'succeeded': return 'Finished';
    case 'failed': return 'Stopped with an error';
    case 'cancelled': return 'Stopped';
    case 'running':
      if (job.stage) return jobStepLabel(job.stage);
      if (isGenerationOperation(job.operation)) return 'Preparing generation';
      if (job.operation.includes('install_models')) return 'Checking cache and downloading weights';
      if (job.operation.endsWith('install_multiview')) return 'Checking cache and downloading multiview weights';
      if (job.operation.endsWith('_setup') || job.operation.endsWith('.setup')) return 'Setting up local runtime';
      if (job.operation.endsWith('convert_naf')) return 'Converting checkpoint';
      return 'Processing model';
  }
}

export function lastGenerationStage(job: Job): string | undefined {
  return isGenerationOperation(job.operation) && job.stage ? jobStepLabel(job.stage) : undefined;
}

function timestamp(value: string | null | undefined): number | null {
  return value && /^\d+$/.test(value) && Number.isSafeInteger(Number(value)) ? Number(value) : null;
}

export function formatDuration(milliseconds: number): string {
  const seconds = Math.floor(Math.max(0, milliseconds) / 1000);
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor(seconds / 3600) % 24;
  const minutes = Math.floor(seconds / 60) % 60;
  if (days) return `${days}d ${hours}h`;
  if (hours) return `${hours}h ${minutes}m`;
  if (minutes) return `${minutes}m ${seconds % 60}s`;
  return `${seconds}s`;
}

export function jobTiming(job: Job, now: number): { label: string; duration: string; queuedFor?: string } {
  const created = timestamp(job.created_at);
  const started = timestamp(job.started_at);
  const end = isActiveJob(job) ? now : timestamp(job.updated_at);
  const start = started ?? created;
  return {
    label: job.state === 'queued' || (job.state === 'cancelled' && started === null)
      ? 'Queue time' : started === null ? 'Since submitted' : 'Elapsed',
    duration: end === null || start === null ? 'Unavailable' : formatDuration(end - start),
    queuedFor: started !== null && created !== null && started - created >= 1000
      ? formatDuration(started - created) : undefined,
  };
}

export function mergeJobRecords(primary: Job[], secondary: Job[]): Job[] {
  const jobs = new Map(primary.map(job => [job.id, job]));
  for (const job of secondary) {
    const prior = jobs.get(job.id);
    if (prior && isActiveJob(prior) && isActiveJob(job) && prior.state !== job.state
      && (prior.state === 'cancelling' || job.state === 'cancelling')) {
      if (job.state === 'cancelling') jobs.set(job.id, job);
      continue;
    }
    if (!prior || (isActiveJob(prior) === isActiveJob(job)
      ? Number(job.updated_at) > Number(prior.updated_at)
      : !isActiveJob(job))) jobs.set(job.id, job);
  }
  return [...jobs.values()].sort((left, right) =>
    Number(isActiveJob(right)) - Number(isActiveJob(left))
    || Number(right.updated_at) - Number(left.updated_at)
    || right.id.localeCompare(left.id));
}
