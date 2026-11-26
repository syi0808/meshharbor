import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog';
import { Spinner } from '@/components/ui/spinner';
import type { MeshHarborApi } from './api';
import { blenderIcon } from './blender-icon';
import { notify } from './notify';
import type { Asset } from './types';

export function BlenderExport({ api, asset, disabled }: { api: MeshHarborApi; asset: Asset; disabled: boolean }) {
  const [open, setOpen] = useState(false);
  const [connected, setConnected] = useState(false);
  const [sending, setSending] = useState(false);
  const [setupAction, setSetupAction] = useState<'install' | 'download' | null>(null);
  const [installedVersion, setInstalledVersion] = useState('');
  const [setupError, setSetupError] = useState('');

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

  async function setup(action: 'install' | 'download') {
    if (setupAction) return;
    setSetupAction(action);
    setSetupError('');
    try {
      if (action === 'install') {
        const { version } = await api.installBlenderAddon();
        setInstalledVersion(version);
        return;
      }
      const blob = await api.downloadBlenderAddon();
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = 'meshharbor_bridge.py';
      anchor.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (cause) {
      setSetupError(cause instanceof Error ? cause.message : 'Could not prepare the Blender add-on.');
    } finally {
      setSetupAction(null);
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
    <DialogContent>
      <DialogHeader>
        <DialogTitle>Send to Blender</DialogTitle>
        <DialogDescription>Add the selected model version to your current Blender scene without saving or replacing the scene.</DialogDescription>
      </DialogHeader>
      <div className="grid gap-2">
        <p role="status" className="font-medium">{connected ? 'Blender is connected.' : 'Waiting for Blender to connect.'}</p>
        {installedVersion && <p role="status">Add-on installed for Blender {installedVersion}. Restart Blender if it is already open.</p>}
        {!connected && <ol className="list-decimal space-y-1 pl-4 text-muted-foreground">
          <li>Install the add-on on this computer, or download it and use <strong>Preferences → Add-ons → Install from Disk</strong> in Blender 4.2 or later.</li>
          <li>Enable <strong>MeshHarbor Bridge</strong> in Blender’s Add-ons preferences.</li>
          <li>In Blender’s 3D Viewport, press <kbd>N</kbd>, open <strong>MeshHarbor</strong>, and select <strong>Start Bridge</strong>.</li>
        </ol>}
        {setupError && <p role="alert" className="text-destructive">{setupError}</p>}
      </div>
      <DialogFooter>
        {connected
          ? <Button disabled={sending || disabled} onClick={() => void send()}>Send to Blender</Button>
          : <>
            <Button variant="outline" disabled={setupAction !== null} onClick={() => void setup('download')}>Download add-on</Button>
            <Button disabled={setupAction !== null} onClick={() => void setup('install')}>
              {setupAction === 'install' && <Spinner />}
              Install locally
            </Button>
          </>}
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}
