import { ClockCounterClockwiseIcon } from '@phosphor-icons/react';
import { useEffect, useRef, useState } from 'react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog';
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Spinner } from '@/components/ui/spinner';
import type { MeshHarborApi } from './api';
import { notify } from './notify';
import type { Asset, ModelVersion, ModelVersionPage } from './types';

export function ModelVersionHistory({ api, model, viewingId, disabled, operationLabel, onView }: {
  api: MeshHarborApi;
  model: NonNullable<Asset['model']>;
  viewingId: string;
  disabled: boolean;
  operationLabel: (operation: string) => string;
  onView: (asset: Asset) => void;
}) {
  const [open, setOpen] = useState(false);
  const [history, setHistory] = useState<ModelVersionPage | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const [retry, setRetry] = useState(0);
  const requests = useRef<AbortController | null>(null);

  useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    requests.current = controller;
    setHistory(null);
    setProblem(null);
    void api.modelVersions(model.id, undefined, controller.signal).then(page => {
      if (!controller.signal.aborted) setHistory(page);
    }).catch(cause => {
      if (!controller.signal.aborted) setProblem(cause instanceof Error ? cause.message : 'Could not load version history.');
    });
    return () => controller.abort();
  }, [api, model.id, model.current_asset_id, open, retry]);

  async function loadOlder() {
    const signal = requests.current?.signal;
    if (!history?.next_before || !signal || working) return;
    setWorking(true);
    try {
      const older = await api.modelVersions(model.id, history.next_before, signal);
      if (!signal.aborted) setHistory({ ...older, items: [...history.items, ...older.items] });
    } catch (cause) {
      if (!signal.aborted) notify('error', cause instanceof Error ? cause.message : 'Could not load older versions.');
    } finally {
      setWorking(false);
    }
  }

  async function viewVersion(version: ModelVersion, restore: boolean) {
    if (working || disabled) return;
    setWorking(true);
    try {
      const asset = restore ? await api.restoreModelVersion(model.id, version.asset_id) : await api.derivedAsset(version.asset_id);
      if (asset.id !== version.asset_id || asset.model?.id !== model.id || (restore && asset.model.current_asset_id !== asset.id)) {
        throw new Error('The version response did not match the selected model.');
      }
      onView(asset);
      setOpen(false);
      if (restore) notify('success', `Version ${version.number} restored. Other versions are unchanged.`);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not open this version.');
    } finally {
      setWorking(false);
    }
  }

  return <Dialog open={open} onOpenChange={next => { if (!working) setOpen(next); }}>
    <DialogTrigger asChild>
      <Button variant="outline" size="sm" disabled={disabled}><ClockCounterClockwiseIcon aria-hidden="true" /> Version history</Button>
    </DialogTrigger>
    <DialogContent className="max-h-[calc(100dvh-2rem)] min-h-0 grid-rows-[auto_minmax(0,1fr)] sm:max-w-lg"
      onEscapeKeyDown={event => { if (working) event.preventDefault(); }}>
      <DialogHeader className="min-w-0 pr-6">
        <DialogTitle>Version history</DialogTitle>
        <DialogDescription className="wrap-anywhere">
          {model.name}. Preview any version, or restore it as the current model. Restoring does not delete later versions.
        </DialogDescription>
      </DialogHeader>
      <ScrollArea type="auto" className="min-h-0 min-w-0" viewportProps={{ className: 'overscroll-contain [&>div]:block!' }}>
        {problem ? <div className="grid justify-items-start gap-2">
          <p role="alert" className="text-destructive">{problem}</p>
          <Button variant="outline" size="sm" onClick={() => setRetry(current => current + 1)}>Retry</Button>
        </div> : !history ? <p role="status" className="flex items-center gap-2"><Spinner aria-hidden="true" /> Loading versions…</p>
          : <div className="grid gap-3 pr-2">
            <ItemGroup aria-label="Model versions">
              {history.items.map(version => {
                const current = version.asset_id === history.current_asset_id;
                const viewing = version.asset_id === viewingId;
                const timestamp = new Date(Number(version.created_at));
                const operation = version.number === 1 ? model.generated ? 'Generated model' : 'Original import'
                  : version.operation ? operationLabel(version.operation) : 'Edited model';
                return <Item key={version.asset_id} role="listitem" variant="outline" size="xs" className="min-w-0 gap-2">
                  <ItemContent className="min-w-0">
                    <ItemTitle className="flex-wrap">Version {version.number}
                      {current && <Badge variant="secondary">Current</Badge>}
                      {viewing && !current && <Badge variant="outline">Viewing</Badge>}
                    </ItemTitle>
                    <ItemDescription className="line-clamp-none wrap-anywhere">
                      {operation}{version.parent_number !== null && ` · From v${version.parent_number}`}
                      {!Number.isNaN(timestamp.getTime()) && ` · ${timestamp.toLocaleString()}`}
                    </ItemDescription>
                  </ItemContent>
                  <ItemActions className="flex-wrap">
                    <Button variant="ghost" size="sm" disabled={working || disabled || viewing}
                      onClick={() => void viewVersion(version, false)} aria-label={`Preview version ${version.number}`}>Preview</Button>
                    {!current && <Button variant="outline" size="sm" disabled={working || disabled}
                      onClick={() => void viewVersion(version, true)} aria-label={`Restore version ${version.number}`}>Restore</Button>}
                  </ItemActions>
                </Item>;
              })}
            </ItemGroup>
            {history.next_before !== null && <Button variant="outline" size="sm" className="justify-self-start"
              onClick={() => void loadOlder()} disabled={working}>Load older versions</Button>}
          </div>}
      </ScrollArea>
    </DialogContent>
  </Dialog>;
}
