import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog';
import { Spinner } from '@/components/ui/spinner';
import type { MeshHarborApi } from './api';
import { blenderIcon } from './blender-icon';
import { notify } from './notify';
import type { Asset } from './types';

type SetupAction = 'install' | 'download';
type SetupState =
  | { stage: 'pending'; action: SetupAction }
  | { stage: 'installed'; version: string }
  | { stage: 'download_started' }
  | { stage: 'failed'; message: string };

export function BlenderExport({ api, asset, disabled }: { api: MeshHarborApi; asset: Asset; disabled: boolean }) {
  const [open, setOpen] = useState(false);
  const [connected, setConnected] = useState(false);
  const [sending, setSending] = useState(false);
  const [setupState, setSetupState] = useState<SetupState | null>(null);
  const setupAction = setupState?.stage === 'pending' ? setupState.action : null;

  useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    let timer = 0;
    async function check() {
      const ready = await api.blenderStatus(controller.signal).then(status => status.connected, () => false);
      if (!controller.signal.aborted) {
        setConnected(ready);
        timer = window.setTimeout(() => void check(), 1000);
      }
    }
    void check();
    return () => {
      controller.abort();
      window.clearTimeout(timer);
    };
  }, [api, open]);

  async function send() {
    if (sending || disabled) return;
    setSending(true);
    const signal = AbortSignal.timeout(260_000);
    try {
      const { connected: ready } = await api.blenderStatus(signal);
      setConnected(ready);
      if (!ready) {
        setOpen(true);
        return;
      }
      let transfer = await api.sendToBlender(asset.id, signal);
      setOpen(false);
      notify('info', 'Sending to Blender…', asset.name);
      while (transfer.stage === 'waiting' || transfer.stage === 'importing') {
        await new Promise(resolve => window.setTimeout(resolve, 850));
        transfer = await api.blenderTransfer(transfer.id, signal);
      }
      if (transfer.stage === 'failed') throw new Error(transfer.error ?? 'Blender could not import the model.');
      notify('success', 'Model added to Blender.', asset.name);
    } catch (cause) {
      notify('error', 'Could not send to Blender.', cause instanceof Error ? cause.message : undefined);
    } finally {
      setSending(false);
    }
  }

  async function setup(action: SetupAction) {
    if (setupAction) return;
    setSetupState({ stage: 'pending', action });
    try {
      if (action === 'install') {
        const { version } = await api.installBlenderAddon();
        setSetupState({ stage: 'installed', version });
        return;
      }
      const blob = await api.downloadBlenderAddon();
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = 'meshharbor_bridge.py';
      anchor.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      setSetupState({ stage: 'download_started' });
    } catch (cause) {
      setSetupState({
        stage: 'failed',
        message: cause instanceof Error ? cause.message : 'Could not prepare the Blender add-on.',
      });
    }
  }

  return <Dialog open={open} onOpenChange={setOpen}>
    <DialogTrigger asChild>
      <Button variant="secondary" size="sm" disabled={disabled} aria-disabled={sending || undefined} aria-busy={sending} onClick={event => {
        event.preventDefault();
        void send();
      }}>
        {sending ? <Spinner /> : <img className="size-4" src={blenderIcon} alt="" />}
        Send to Blender
      </Button>
    </DialogTrigger>
    <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto">
      <DialogHeader>
        <DialogTitle>Send to Blender</DialogTitle>
        <DialogDescription>Add the selected model version to your current Blender scene without saving or replacing the scene.</DialogDescription>
      </DialogHeader>
      <div className="grid gap-2">
        {!connected && <ol className="list-decimal space-y-2 pl-4 text-muted-foreground">
          <li>
            Choose one installation method for Blender 4.2 or later:
            <ul className="mt-1 list-disc space-y-1 pl-4">
              <li><strong>Install locally (recommended):</strong> Quit Blender first. MeshHarbor finds Blender on this computer, installs the add-on, and enables it for the next launch.</li>
              <li><strong>Download add-on (manual):</strong> Use this if automatic installation fails or MeshHarbor runs in Docker. In Blender, choose <strong>Preferences → Add-ons → Install from Disk</strong> and select the downloaded file.</li>
            </ul>
          </li>
          <li>Open Blender. If you downloaded the add-on, enable <strong>MeshHarbor Bridge</strong> in Blender’s Add-ons preferences.</li>
          <li>In Blender’s 3D Viewport, press <kbd>N</kbd>, open <strong>MeshHarbor</strong>, and select <strong>Start Bridge</strong>.</li>
        </ol>}
        <div role="status" className={connected ? 'grid gap-1' : 'grid gap-1 border-t pt-2'}>
          {!connected && <>
            {setupAction === 'install' && <p>Checking Blender and preparing the add-on…</p>}
            {setupAction === 'download' && <p>Preparing the add-on download…</p>}
            {setupState?.stage === 'installed' && <p>Add-on installed and enabled for Blender {setupState.version}. Open Blender to start the bridge.</p>}
            {setupState?.stage === 'download_started' && <p>Download started. Install the downloaded file in Blender to continue.</p>}
          </>}
          <p className="font-medium">{connected ? 'Blender is connected.' : <>
            Waiting for Blender to connect<span className="sr-only">.</span><span aria-hidden="true" className="inline-block motion-safe:animate-blender-waiting-dots after:content-['...']" />
          </>}</p>
        </div>
        {!connected && setupState?.stage === 'failed' && <p role="alert" className="text-destructive">{setupState.message}</p>}
      </div>
      <DialogFooter>
        {connected
          ? <Button disabled={sending || disabled} onClick={() => void send()}>Send to Blender</Button>
          : <>
            <Button variant="outline" disabled={setupAction !== null} onClick={() => void setup('download')}>
              {setupAction === 'download' ? <><Spinner /> Downloading…</> : 'Download add-on'}
            </Button>
            <Button disabled={setupAction !== null} onClick={() => void setup('install')}>
              {setupAction === 'install' ? <><Spinner /> Installing…</> : 'Install locally'}
            </Button>
          </>}
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}
