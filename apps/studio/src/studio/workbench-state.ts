export type Inspector = 'generation' | 'optimize' | 'tools' | 'jobs';

export const DESKTOP_SIDEBAR_KEY = 'meshharbor.studio.sidebar.v1';
export const WORKBENCH_NARROW_QUERY = '(max-width: 760px)';

export function decodeInventoryOpen(value: string | null): boolean {
  return value === null || value === 'assets' || value === 'jobs';
}

export function runtimeSetupStage(backend: {
  installable: boolean;
  ready?: boolean;
  environment_ready?: boolean;
  models_downloaded?: boolean;
  naf_conversion_required?: boolean;
}): 'ready' | 'unsupported' | 'environment' | 'models' | 'conversion' | 'incomplete' {
  if (backend.ready) return 'ready';
  if (!backend.installable) return 'unsupported';
  if (!backend.environment_ready) return 'environment';
  if (!backend.models_downloaded) return 'models';
  return backend.naf_conversion_required ? 'conversion' : 'incomplete';
}

export function shouldHighlightExport(
  activeVersion: 'source' | 'result',
  isRunning: boolean,
  connected: boolean,
): boolean {
  return activeVersion === 'result' && !isRunning && connected;
}

export function formatVersionSizes(original: number, result: number): string | null {
  if (![original, result].every(value => Number.isSafeInteger(value) && value >= 0)) return null;
  // Both versions use the same unit. Give a precise byte delta when rounding hides a change.
  const max = Math.max(original, result);
  const [scale, unit] = max >= 1024 ** 2 ? [1024 ** 2, 'MiB'] : max >= 1024 ? [1024, 'KiB'] : [1, 'B'];
  const decimals = scale === 1 ? 0 : 2;
  const first = (original / scale).toFixed(decimals);
  const second = (result / scale).toFixed(decimals);
  const delta = result - original;
  const roundedChange = first === second && delta !== 0
    ? ` (${delta > 0 ? '+' : '−'}${Math.abs(delta).toLocaleString('en-US')} B)` : '';
  return `${first} → ${second} ${unit}${roundedChange}`;
}
