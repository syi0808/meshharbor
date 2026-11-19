import { ArrowLeftIcon, ArrowsClockwiseIcon, CubeIcon, HardDrivesIcon, KeyIcon } from '@phosphor-icons/react';
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Field, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import type { MeshHarborApi } from './api';
import type { HuggingFaceAccess, Job, RuntimeBackendId, RuntimeInfo } from './types';
import { runtimeSetupStage } from './workbench-state';

const sections = {
  models: { title: 'Models', icon: CubeIcon, description: 'Browse the generation models supported by MeshHarbor and prepare them for this computer.' },
  connections: { title: 'Connections', icon: KeyIcon, description: 'Manage the credentials MeshHarbor uses to download gated model files.' },
  storage: { title: 'Downloads & storage', icon: HardDrivesIcon, description: 'Install or remove local model files and review recent setup and download activity.' },
};

export type SettingsSection = keyof typeof sections;
export type RuntimeAction = 'setup' | 'download' | 'convert';

const sectionIds: SettingsSection[] = ['models', 'connections', 'storage'];
const profiles = {
  'pixal3d-mlx': { title: 'Apple MLX', platform: 'macOS · Apple Silicon', download: '~24 GB', free: '25 GB' },
  'pixal3d-cuda': { title: 'NVIDIA CUDA', platform: 'Linux x86-64 · NVIDIA GPU', download: '25–30 GB', free: '40 GiB' },
} satisfies Record<RuntimeBackendId, object>;

const profileIds: RuntimeBackendId[] = ['pixal3d-mlx', 'pixal3d-cuda'];
type Backend = RuntimeInfo['model_backends'][number];
type RemovalScope = 'huggingface' | 'model-files';
type AccessStatus = HuggingFaceAccess | 'checking';
type VisitSettings = (section: SettingsSection, backend?: RuntimeBackendId) => void;

export function settingsSectionFromPath(path: string): SettingsSection | null {
  const [, base, section] = path.replace(/\/$/, '').split('/');
  if (base !== 'settings') return null;
  return sectionIds.find(candidate => candidate === section) ?? 'models';
}

interface Props {
  api: MeshHarborApi;
  section: SettingsSection;
  backendId: RuntimeBackendId | null;
  runtime: RuntimeInfo | null;
  connection: 'disconnected' | 'connecting' | 'connected';
  connectionError: string | null;
  busy: boolean;
  feedback: ReactNode;
  jobFeedback: ReactNode;
  jobs: Job[];
  operationLabel: (operation: string) => string;
  onVisit: VisitSettings;
  onProfileChange: (backend: RuntimeBackendId) => void;
  onClose: () => void;
  onReconnect: () => void;
  onRefreshJobs: () => Promise<void>;
  onRuntimeChange: (runtime: RuntimeInfo) => void;
  onRuntimeAction: (backend: RuntimeBackendId, action: RuntimeAction) => void;
}

export function SettingsPage({ api, section, backendId, runtime, connection, connectionError, busy, feedback, jobFeedback, jobs, operationLabel, onVisit, onProfileChange, onClose, onReconnect, onRefreshJobs, onRuntimeChange, onRuntimeAction }: Props) {
  const heading = useRef<HTMLHeadingElement>(null);
  const cancelRemoval = useRef<HTMLButtonElement>(null);
  const removalTrigger = useRef<HTMLElement | null>(null);
  const [access, setAccess] = useState<Partial<Record<RuntimeBackendId, AccessStatus>>>({});
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [removal, setRemoval] = useState<{ backend: RuntimeBackendId; scope: RemovalScope } | null>(null);
  const selectedId = backendId ?? (runtime?.platform === 'linux' ? 'pixal3d-cuda' : 'pixal3d-mlx');
  const selected = runtime?.model_backends.find(backend => backend.id === selectedId);
  const disabled = pending || busy;
  const sectionInfo = sections[section];
  const runtimeJobs = jobs.filter(job => job.operation.startsWith('runtime.')).slice(0, 6);
  useEffect(() => { heading.current?.focus(); }, [section]);

  async function perform(action: () => Promise<void>) {
    setPending(true);
    setError('');
    setNotice('');
    try {
      await action();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not update local settings.');
    } finally {
      setPending(false);
    }
  }

  function verify(backend: RuntimeBackendId, token?: string) {
    void perform(async () => {
      setAccess(current => ({ ...current, [backend]: 'checking' }));
      try {
        const result = await api.checkHuggingFaceAccess(backend, token);
        setAccess(current => ({ ...current, [backend]: result }));
      } catch (cause) {
        setAccess(current => ({ ...current, [backend]: undefined }));
        throw cause;
      }
      onRuntimeChange(await api.runtime());
    });
  }

  function confirmRemoval() {
    if (!removal) return;
    const { backend, scope } = removal;
    void perform(async () => {
      try {
        onRuntimeChange(await api.removeRuntimeFiles(backend, scope));
        if (scope === 'huggingface') setAccess(current => ({ ...current, [backend]: 'not_connected' }));
        setNotice(scope === 'huggingface'
          ? `Hugging Face disconnected for ${profiles[backend].title} on this computer.`
          : `Pixal3D model files removed for ${profiles[backend].title}.`);
      } finally {
        setRemoval(null);
      }
    });
  }

  function openRemoval(backend: RuntimeBackendId, scope: RemovalScope) {
    removalTrigger.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setRemoval({ backend, scope });
  }

  return <div className="min-h-dvh bg-background text-foreground">
    <header className="border-b bg-background">
      <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-3 px-4 py-2 sm:px-6">
        <Button variant="ghost" size="sm" onClick={onClose}><ArrowLeftIcon aria-hidden="true" /> Workbench</Button>
        <span className="border-l pl-3 font-heading text-sm font-medium">Settings</span>
        <Badge className="ml-auto" variant="outline">{connection === 'connected' ? 'Engine connected' : connection === 'connecting' ? 'Connecting…' : 'Engine offline'}</Badge>
      </div>
    </header>
    <div className="mx-auto grid max-w-6xl gap-6 px-4 py-5 sm:px-6 md:grid-cols-[12rem_minmax(0,1fr)] md:gap-8 md:py-8">
      <nav aria-label="Settings sections" className="flex flex-wrap content-start gap-1 md:flex-col">
        {sectionIds.map(id => {
          const { title, icon: Icon } = sections[id];
          return <Button key={id} size="lg" className="justify-start" variant={section === id ? 'secondary' : 'ghost'}
            aria-current={section === id ? 'page' : undefined} onClick={() => onVisit(id)}>
            <Icon data-icon="inline-start" aria-hidden="true" />{title}
          </Button>;
        })}
      </nav>
      <main className="grid min-w-0 content-start gap-5 text-sm" aria-labelledby="settings-heading" data-connection={connection}>
        <div className="flex flex-wrap items-start justify-between gap-3 border-b pb-4">
          <div className="grid gap-1">
            <h1 ref={heading} tabIndex={-1} id="settings-heading" className="font-heading text-2xl font-semibold outline-none">{sectionInfo.title}</h1>
            <p className="text-muted-foreground">{sectionInfo.description}</p>
          </div>
          {connection === 'connected' && <Button variant="outline" onClick={() => void perform(async () => {
            onRuntimeChange(await api.runtime());
            await onRefreshJobs();
          })} disabled={disabled}><ArrowsClockwiseIcon aria-hidden="true" /> Refresh status</Button>}
        </div>
        {feedback}
        {jobFeedback}
        {notice && <Alert role="status"><AlertDescription>{notice}</AlertDescription></Alert>}
        {error && <Alert variant="destructive" role="alert"><AlertDescription>{error}</AlertDescription></Alert>}
        {connection !== 'connected' ? <section className="grid justify-items-start gap-2 rounded-lg border bg-card p-4">
          <h2 className="font-medium">{connection === 'connecting' ? 'Connecting to the local engine' : 'Local engine is offline'}</h2>
          <p className="text-muted-foreground">Start <code>meshharbor serve</code> to view and manage local models.</p>
          {connectionError && <p className="wrap-anywhere text-muted-foreground">{connectionError}</p>}
          <Button disabled={connection === 'connecting'} onClick={onReconnect}>Retry connection</Button>
        </section> : <>
          {section === 'models' && (runtime?.model_catalog ?? []).map(model => <section key={model.id} className="grid gap-4 rounded-lg border bg-card p-4 sm:p-5">
            <div className="grid gap-1">
              <div className="flex flex-wrap items-center gap-2">
                <h2 className="font-heading text-lg font-medium">{model.title}</h2>
                <Badge variant="outline">Image to 3D</Badge>
                {model.status === 'experimental_opt_in' && <Badge variant="outline">Experimental</Badge>}
              </div>
              <p className="text-muted-foreground">Generate a textured GLB from a reference image. Once installed, generation runs locally.</p>
            </div>
            <div className="grid gap-3 lg:grid-cols-2">
              {profileIds.filter(id => model.backend_ids.includes(id)).map(id => {
                const backend = runtime?.model_backends.find(item => item.id === id);
                if (!backend) return null;
                return <article key={id} className="grid content-start gap-3 rounded-md border bg-background p-3">
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div><h3 className="font-medium">{profiles[id].title}</h3><p className="text-xs text-muted-foreground">{profiles[id].platform}</p></div>
                    <Badge variant={backend.ready ? 'secondary' : 'outline'}>{profileStatus(backend)}</Badge>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {runtimeSetupStage(backend) === 'environment' && <Button onClick={() => onRuntimeAction(id, 'setup')} disabled={disabled}>Set up {id === 'pixal3d-cuda' ? 'CUDA' : 'Python'} environment</Button>}
                    {(backend.installable || backend.local_files?.remove_models_available) && <Button variant="outline" onClick={() => onVisit('storage', id)}>{backend.ready ? 'Manage files' : 'View downloads'}</Button>}
                  </div>
                </article>;
              })}
            </div>
          </section>)}
          {section === 'models' && !runtime?.model_catalog?.length && <p className="text-muted-foreground">No generation models reported by the local engine.</p>}
          {section !== 'models' && <Field className="max-w-xs">
            <FieldLabel htmlFor="settings-backend">Backend</FieldLabel>
            <NativeSelect id="settings-backend" className="w-full" value={selectedId} onChange={event => {
              const choice = profileIds.find(id => id === event.target.value);
              if (choice) onProfileChange(choice);
            }}>
              {profileIds.map(id => <NativeSelectOption value={id} key={id}>{profiles[id].title}</NativeSelectOption>)}
            </NativeSelect>
          </Field>}
          {section !== 'models' && !selected && <p className="text-muted-foreground">The local engine did not report this backend.</p>}
          {section === 'connections' && selected && <HuggingFaceConnection key={selectedId}
            backend={selected} access={access[selectedId]} disabled={disabled}
            onCheck={token => verify(selectedId, token)}
            onDisconnect={() => openRemoval(selectedId, 'huggingface')}
            onNext={() => onVisit('storage', selectedId)} />}
          {section === 'storage' && selected && <ModelStorage key={selectedId} backend={selected} id={selectedId}
            access={access[selectedId]} disabled={disabled}
            onDownload={() => onRuntimeAction(selectedId, 'download')} onConvert={() => onRuntimeAction(selectedId, 'convert')}
            onSettings={onVisit} onCheck={() => verify(selectedId)}
            onRemove={() => openRemoval(selectedId, 'model-files')} />}
          {section === 'storage' && <section className="grid gap-2" aria-labelledby="runtime-activity-heading">
            <h2 id="runtime-activity-heading" className="font-heading text-base font-medium">Recent activity</h2>
            {runtimeJobs.length ? <ul className="divide-y rounded-lg border bg-card">
              {runtimeJobs.map(job => <li className="grid gap-1 px-3 py-2" key={job.id}>
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span>{operationLabel(job.operation)}</span><Badge variant="outline">{job.state === 'succeeded' ? 'Complete' : job.state}</Badge>
                </div>
                {job.error?.message && <p className="wrap-anywhere text-xs text-muted-foreground">{job.error.message}</p>}
              </li>)}
            </ul> : <p className="text-muted-foreground">No setup or download jobs yet.</p>}
          </section>}
        </>}
      </main>
    </div>
    <Dialog open={removal !== null} onOpenChange={open => { if (!open && !pending) setRemoval(null); }}>
      <DialogContent showCloseButton={false} onOpenAutoFocus={event => {
        event.preventDefault();
        cancelRemoval.current?.focus();
      }} onCloseAutoFocus={event => {
        event.preventDefault();
        const trigger = removalTrigger.current;
        if (trigger?.isConnected) trigger.focus();
        else heading.current?.focus();
      }}>
        <DialogHeader>
          <DialogTitle>{removal?.scope === 'huggingface' ? 'Disconnect Hugging Face?' : 'Remove model files?'}</DialogTitle>
          <DialogDescription>{removal && (removal.scope === 'huggingface'
            ? `Remove the locally saved Hugging Face credentials for ${profiles[removal.backend].title}. This does not revoke the token on Hugging Face or remove downloaded models.`
            : `Remove Pixal3D weights and transfer caches for ${profiles[removal.backend].title}. The Python environment, Hugging Face connection, and all Studio assets stay in place.`)}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <DialogClose asChild><Button ref={cancelRemoval} variant="outline" disabled={pending}>Cancel</Button></DialogClose>
          <Button variant="destructive" onClick={confirmRemoval} disabled={disabled}>{pending ? 'Removing…' : removal?.scope === 'huggingface' ? 'Disconnect' : 'Remove files'}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>;
}

function profileStatus(backend: Backend) {
  if (backend.ready) return 'Ready';
  if (!backend.installable) return 'Not supported here';
  if (!backend.environment_ready) return 'Setup required';
  if (backend.models_downloaded) return 'Files present';
  return 'Download required';
}

const accessMessages: Record<AccessStatus, string> = {
  checking: 'Checking DINOv3 file access…',
  verified: 'DINOv3 file access verified.',
  not_connected: 'No token is saved for this backend. Create and connect one above.',
  access_denied: 'DINOv3 access denied. Check your approval on Hugging Face and use a token with gated read access.',
};

function HuggingFaceConnection({ backend, access, disabled, onCheck, onDisconnect, onNext }: {
  backend: Backend; access?: AccessStatus; disabled: boolean;
  onCheck: (token?: string) => void; onDisconnect: () => void; onNext: () => void;
}) {
  const local = backend.local_files;
  const canConnect = backend.installable && backend.environment_ready;
  function connectToken(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const token = new FormData(event.currentTarget).get('token');
    event.currentTarget.reset();
    if (typeof token === 'string') onCheck(token);
  }
  return <section className="grid gap-4 rounded-lg border bg-card p-4 sm:p-5" aria-label="Hugging Face connection">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <div><h2 className="font-heading text-base font-medium">Hugging Face</h2><p className="text-muted-foreground">DINOv3 checkpoint access · credentials are separate for each backend.</p></div>
      <Badge variant="outline">{access === 'verified' ? 'Access verified' : local?.token_saved ? 'Token saved' : local?.credentials_managed === false ? 'Custom storage' : 'Not connected'}</Badge>
    </div>
    {canConnect ? <>
      <ol className="grid list-decimal gap-2 pl-5 text-sm">
        <li><a className="text-primary underline underline-offset-4" href="https://huggingface.co/facebook/dinov3-vitl16-pretrain-lvd1689m" target="_blank" rel="noreferrer">Review terms and request DINOv3 access</a> on Hugging Face.</li>
        <li><a className="text-primary underline underline-offset-4" href="https://huggingface.co/settings/tokens" target="_blank" rel="noreferrer">Create a Hugging Face token</a> in the same account with read access to public gated repositories.</li>
      </ol>
      <form method="post" autoComplete="off" className="grid gap-2" onSubmit={connectToken}>
        <Label htmlFor="huggingface-token">Hugging Face access token</Label>
        <Input id="huggingface-token" name="token" type="password" placeholder="hf_…" maxLength={512} required disabled={disabled} autoComplete="off" className="max-w-md" />
        <p className="text-xs text-muted-foreground">New tokens replace the active token only after MeshHarbor verifies access to the checkpoint file.</p>
        <div className="flex flex-wrap gap-2">
          <Button type="submit" disabled={disabled}>{local?.token_saved ? 'Replace token' : 'Connect token'}</Button>
          <Button type="button" variant="outline" disabled={disabled} onClick={() => onCheck()}>Check saved token</Button>
          {access === 'verified' && <Button type="button" variant="outline" onClick={onNext}>View downloads</Button>}
        </div>
      </form>
    </> : <p className="text-muted-foreground">{backend.installable ? 'Set up the Python environment in Models before connecting a token.' : 'This backend cannot verify a token on this computer.'}</p>}
    {access && <p role="status" className="text-muted-foreground">{accessMessages[access]}</p>}
    {local?.disconnect_available && <div className="border-t pt-3"><Button variant="destructive" disabled={disabled} onClick={onDisconnect}>Disconnect locally</Button></div>}
    {local?.credentials_managed === false && <p className="text-muted-foreground">Studio cannot remove credentials from custom or linked runtime directories.</p>}
  </section>;
}

function ModelStorage({ backend, id, access, disabled, onDownload, onConvert, onSettings, onCheck, onRemove }: {
  backend: Backend; id: RuntimeBackendId; access?: AccessStatus; disabled: boolean;
  onDownload: () => void; onConvert: () => void; onSettings: VisitSettings; onCheck: () => void; onRemove: () => void;
}) {
  const [consent, setConsent] = useState(false);
  const stage = runtimeSetupStage(backend);
  const local = backend.local_files;
  const verificationRequired = access !== 'verified';
  return <section className="grid gap-4 rounded-lg border bg-card p-4 sm:p-5" aria-label="Model files">
    <div className="flex flex-wrap items-start justify-between gap-2">
      <div><h2 className="font-heading text-base font-medium">Pixal3D · {profiles[id].title}</h2><p className="text-muted-foreground">{profiles[id].download} download · allow around {profiles[id].free} free</p></div>
      <Badge variant={backend.ready ? 'secondary' : 'outline'}>{backend.ready ? 'Installed' : backend.models_downloaded ? 'Files present' : local?.remove_models_available ? 'Cached files present' : 'Not installed'}</Badge>
    </div>
    {stage === 'unsupported' && <p className="text-muted-foreground">This backend is not supported on this computer.</p>}
    {stage === 'environment' && <div className="flex flex-wrap items-center gap-2"><p className="text-muted-foreground">Set up the Python environment first.</p><Button variant="outline" onClick={() => onSettings('models')}>View model setup</Button></div>}
    {stage === 'models' && <div className="grid justify-items-start gap-3">
      <p className="text-muted-foreground">Review the model and checkpoint terms before downloading. Interrupted transfers can reuse cached files.</p>
      {verificationRequired && <div className="flex flex-wrap gap-2">
        <Button variant="outline" onClick={() => onSettings('connections', id)}>Manage Hugging Face connection</Button>
        {local?.token_saved && <Button variant="outline" disabled={disabled} onClick={() => { setConsent(false); onCheck(); }}>Verify saved token</Button>}
      </div>}
      {access && <p role="status" className="text-muted-foreground">{accessMessages[access]}</p>}
      <div className="flex items-start gap-2">
        <Checkbox id="model-download-consent" checked={consent && !verificationRequired}
          disabled={disabled || verificationRequired || !backend.downloads_enabled} onCheckedChange={value => setConsent(value === true)} />
        <Label htmlFor="model-download-consent" className="leading-relaxed">I reviewed the model terms and approve this download.</Label>
      </div>
      <Button disabled={disabled || verificationRequired || !consent || !backend.downloads_enabled} onClick={() => { setConsent(false); onDownload(); }}>Download model files</Button>
    </div>}
    {stage === 'conversion' && <div className="grid justify-items-start gap-2">
      <p className="text-muted-foreground">Finish the one-time NAF conversion before using this model.</p>
      <Button disabled={disabled || !backend.conversion_enabled} onClick={onConvert}>Finish NAF conversion</Button>
    </div>}
    {stage === 'incomplete' && <p className="text-muted-foreground">The files are present, but runtime checks have not passed. Run <code>meshharbor doctor</code> for details.</p>}
    {stage === 'ready' && <p className="text-muted-foreground">Ready for offline generation in the workbench.</p>}
    {local?.remove_models_available && <div className="grid justify-items-start gap-2 border-t pt-3">
      <p className="text-muted-foreground">Removing model files does not remove the Python environment, saved credentials, or assets.</p>
      <Button variant="destructive" disabled={disabled} onClick={onRemove}>{backend.models_downloaded ? 'Uninstall model files' : 'Clear cached model files'}</Button>
    </div>}
    {local?.model_files_managed === false && <p className="text-muted-foreground">Studio cannot remove files from custom or linked model directories.</p>}
  </section>;
}
