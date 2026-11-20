import { useEffect, useState } from 'react';
import type { MeshHarborApi } from './api';
import { notify } from './notify';
import type { RetopologyPreflight } from './types';

export function useRetopologyPreflight(api: MeshHarborApi, assetId: string | null, targetQuads: number) {
  const [result, setResult] = useState<{ assetId: string; targetQuads: number; report: RetopologyPreflight | null } | null>(null);

  useEffect(() => {
    setResult(null);
    if (assetId === null) return;

    const controller = new AbortController();
    const timer = window.setTimeout(() => {
      void api.preflightRetopology(assetId, targetQuads, controller.signal).then(report => {
        if (!controller.signal.aborted) setResult({ assetId, targetQuads, report });
      }).catch(cause => {
        if (controller.signal.aborted) return;
        setResult({ assetId, targetQuads, report: null });
        const detail = cause instanceof Error ? cause.message.replace(/^operation unavailable:\s*/i, '') : undefined;
        notify('error', 'Retopology input check failed.', detail);
      });
    }, 300);

    return () => {
      window.clearTimeout(timer);
      controller.abort();
    };
  }, [api, assetId, targetQuads]);

  const current = result?.assetId === assetId && result.targetQuads === targetQuads ? result : null;
  return { checking: assetId !== null && current === null, report: current?.report ?? null };
}
