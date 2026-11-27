import type { RuntimeInfo } from './types';

type Backend = RuntimeInfo['model_backends'][number];

export function preferredModelBackend(backends: Backend[], platform: string, isReady: (backend: Backend) => boolean) {
  const preferred = platform === 'linux' ? '-cuda' : '-mlx';
  return backends.find(backend => isReady(backend) && backend.id.endsWith(preferred))
    ?? backends.find(isReady) ?? backends.find(backend => backend.installable) ?? backends[0];
}
