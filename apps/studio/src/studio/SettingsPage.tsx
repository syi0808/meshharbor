import { ArrowLeftIcon, ArrowsClockwiseIcon, CubeIcon, HardDrivesIcon, KeyIcon, PaintBrushIcon } from '@phosphor-icons/react';
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
import { Disclosure } from './Disclosure';
import { notify } from './notify';
import type { MeshHarborApi } from './api';
import type { HuggingFaceAccess, HuggingFaceBrowserLogin, HuggingFaceConnectionStatus, Job, RuntimeBackendId, RuntimeInfo } from './types';
import { runtimeSetupStage } from './workbench-state';
import type { StudioTheme } from './theme';

const sections = {
  models: { title: 'Models', icon: CubeIcon, description: 'Browse the generation models supported by MeshHarbor and prepare them for this computer.' },
  connections: { title: 'Connections', icon: KeyIcon, description: 'Manage local sign-in for external services.' },
  storage: { title: 'Downloads & storage', icon: HardDrivesIcon, description: 'Install or remove local model files and review recent setup and download activity.' },
  preferences: { title: 'Preferences', icon: PaintBrushIcon, description: 'Choose how Studio looks on this device.' },
};

export type SettingsSection = keyof typeof sections;
export type RuntimeAction = 'setup' | 'download' | 'convert';

const sectionIds: SettingsSection[] = ['models', 'connections', 'storage', 'preferences'];
const profiles = {
  'pixal3d-mlx': { title: 'Apple MLX', platform: 'macOS · Apple Silicon', download: '24 GB' },
  'pixal3d-cuda': { title: 'NVIDIA CUDA', platform: 'Linux x86-64 · NVIDIA GPU', download: '30 GB' },
} satisfies Record<RuntimeBackendId, object>;

const profileIds: RuntimeBackendId[] = ['pixal3d-mlx', 'pixal3d-cuda'];
type Backend = RuntimeInfo['model_backends'][number];
type RemovalScope = 'huggingface' | 'model-files';
type AccessStatus = HuggingFaceAccess | 'checking' | 'unavailable';
type AccountStatus = HuggingFaceConnectionStatus | 'checking';
type VisitSettings = (section: SettingsSection, backend?: RuntimeBackendId) => void;

export function settingsSectionFromPath(path: string): SettingsSection | null {
  const [, base, section] = path.replace(/\/$/, '').split('/');
  if (base !== 'settings') return null;
  return sectionIds.find(candidate => candidate === section) ?? 'models';
}

interface Props {
  api: MeshHarborApi;
  section: SettingsSection | null;
  backendId: RuntimeBackendId | null;
  runtime: RuntimeInfo | null;
  connection: 'disconnected' | 'connecting' | 'connected';
  connectionError: string | null;
  busy: boolean;
  jobFeedback: ReactNode;
  theme: StudioTheme;
  jobs: Job[];
  operationLabel: (operation: string) => string;
  onVisit: VisitSettings;
  onProfileChange: (backend: RuntimeBackendId) => void;
  onClose: () => void;
  onReconnect: () => void;
  onRefreshJobs: () => Promise<boolean>;
  onThemeChange: (theme: StudioTheme) => void;
  onRuntimeChange: (runtime: RuntimeInfo) => void;
  onRuntimeAction: (backend: RuntimeBackendId, action: RuntimeAction) => void;
}

export function SettingsPage({ api, section, backendId, runtime, connection, connectionError, busy, jobFeedback, theme, jobs, operationLabel, onVisit, onProfileChange, onClose, onReconnect, onRefreshJobs, onThemeChange, onRuntimeChange, onRuntimeAction }: Props) {
  const heading = useRef<HTMLHeadingElement>(null);
  const cancelRemoval = useRef<HTMLButtonElement>(null);
  const removalTrigger = useRef<HTMLElement | null>(null);
  const [accounts, setAccounts] = useState<Partial<Record<RuntimeBackendId, AccountStatus>>>({});
  const [pending, setPending] = useState(false);
  const [removal, setRemoval] = useState<{ backend: RuntimeBackendId; scope: RemovalScope } | null>(null);
  const [browserLogin, setBrowserLogin] = useState<{ backend: RuntimeBackendId; session: HuggingFaceBrowserLogin } | null>(null);
  const selectedId = backendId ?? (runtime?.platform === 'linux' ? 'pixal3d-cuda' : 'pixal3d-mlx');
  const selected = runtime?.model_backends.find(backend => backend.id === selectedId);
  const browserBackend = browserLogin?.backend;
  const browserSessionId = browserLogin?.session.id;
  const browserActive = browserLogin && browserLoginIsActive(browserLogin.session);
  const disabled = pending || busy || Boolean(browserActive);
  const runtimeJobs = jobs.filter(job => job.operation.startsWith('runtime.')).slice(0, 6);
  useEffect(() => {
    if (section) heading.current?.focus();
    else setRemoval(null);
  }, [section]);
  useEffect(() => {
    if (!browserBackend || !browserSessionId) return;
    let stopped = false;
    let timer = 0;
    let failures = 0;
    async function poll(backend: RuntimeBackendId, id: string) {
      try {
        const session = await api.huggingFaceBrowserLogin(backend, id);
        if (stopped) return;
        failures = 0;
        setBrowserLogin({ backend, session });
        if (browserLoginIsActive(session)) {
          timer = window.setTimeout(() => void poll(backend, id), 1000);
          return;
        }
        if (session.state === 'signed_in') {
          setAccounts(current => ({ ...current, [backend]: 'connected' }));
          try {
            const nextRuntime = await api.runtime();
            if (stopped) return;
            onRuntimeChange(nextRuntime);
            notify('success', `Signed in to Hugging Face for ${profiles[backend].title}.`);
          } catch {
            if (!stopped) notify('warning', 'Signed in, but the local status could not be refreshed. Select Refresh status to retry.');
          }
        } else if (session.state === 'failed' || session.state === 'expired') {
          notify('error', session.state === 'failed' ? session.message : 'The Hugging Face code expired. Start browser sign-in again.');
        } else if (session.state === 'cancelled') {
          notify('info', 'Hugging Face sign-in cancelled.');
        }
      } catch {
        if (stopped) return;
        if (++failures < 3) {
          timer = window.setTimeout(() => void poll(backend, id), 1000);
        } else {
          setBrowserLogin(null);
          notify('error', 'Could not check browser sign-in. Select browser sign-in again to reconnect.');
        }
      }
    }
    void poll(browserBackend, browserSessionId);
    return () => { stopped = true; window.clearTimeout(timer); };
  }, [api, browserBackend, browserSessionId, onRuntimeChange]);

  async function perform(action: () => Promise<void>) {
    setPending(true);
    try {
      await action();
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not update local settings.');
    } finally {
      setPending(false);
    }
  }

  function verifyConnection(backend: RuntimeBackendId, token?: string) {
    void perform(async () => {
      setAccounts(current => ({ ...current, [backend]: 'checking' }));
      let result: HuggingFaceConnectionStatus;
      try {
        result = await api.checkHuggingFaceConnection(backend, token);
        setAccounts(current => ({ ...current, [backend]: result }));
      } catch (cause) {
        setAccounts(current => ({ ...current, [backend]: undefined }));
        throw cause;
      }
      if (token !== undefined && result === 'connected') {
        try {
          onRuntimeChange(await api.runtime());
        } catch {
          notify('warning', 'Token connected, but the local status could not be refreshed.');
          return;
        }
      }
      notify(result === 'connected' ? 'success' : result === 'invalid_token' ? 'error' : 'warning', accountMessages[result]);
    });
  }

  function startBrowserLogin(backend: RuntimeBackendId) {
    void perform(async () => {
      const session = await api.startHuggingFaceBrowserLogin(backend);
      setBrowserLogin({ backend, session });
    });
  }

  function cancelBrowserLogin() {
    if (!browserLogin) return;
    void perform(async () => {
      await api.huggingFaceBrowserLogin(browserLogin.backend, browserLogin.session.id, true);
    });
  }

  function confirmRemoval() {
    if (!removal) return;
    const { backend, scope } = removal;
    void perform(async () => {
      try {
        onRuntimeChange(await api.removeRuntimeFiles(backend, scope));
        if (scope === 'huggingface') {
          setAccounts(current => ({ ...current, [backend]: 'not_connected' }));
        }
        notify('success', scope === 'huggingface'
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

  if (!section) return null;
  const sectionInfo = sections[section];

  return <div className="min-h-dvh bg-background text-foreground">
    <header className="border-b bg-background">
      <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-3 px-4 py-2 sm:px-6">
        <Button variant="ghost" size="sm" onClick={onClose}><ArrowLeftIcon aria-hidden="true" /> Workbench</Button>
        <span className="border-l pl-3 font-heading text-sm font-medium">Settings</span>
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
          {section !== 'preferences' && connection === 'connected' && <Button variant="outline" onClick={() => void perform(async () => {
            onRuntimeChange(await api.runtime());
            setAccounts({});
            await onRefreshJobs();
            notify('success', 'Local status refreshed.');
          })} disabled={disabled}><ArrowsClockwiseIcon aria-hidden="true" /> Refresh status</Button>}
        </div>
        {section !== 'connections' && section !== 'preferences' && jobFeedback}
        {section !== 'connections' && browserActive && <Alert role="status">
          <AlertDescription>Hugging Face sign-in is in progress.</AlertDescription>
          <Button variant="outline" onClick={() => onVisit('connections', browserBackend)}>Return to sign-in</Button>
        </Alert>}
        {section === 'preferences' ? <section className="grid gap-4 rounded-lg border bg-card p-4 sm:p-5">
          <h2 className="font-heading text-base font-medium">Appearance</h2>
          <Field className="max-w-xs">
            <FieldLabel htmlFor="studio-theme">Theme</FieldLabel>
            <NativeSelect id="studio-theme" className="w-full" value={theme} onChange={event => {
              const choice = event.target.value;
              if (choice === 'dark' || choice === 'light') {
                onThemeChange(choice);
                notify('success', choice === 'dark' ? 'Dark theme enabled.' : 'Light theme enabled.');
              }
            }}>
              <NativeSelectOption value="dark">Dark</NativeSelectOption>
              <NativeSelectOption value="light">Light</NativeSelectOption>
            </NativeSelect>
          </Field>
        </section> : connection !== 'connected' ? <section className="grid justify-items-start gap-2 rounded-lg border bg-card p-4">
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
            }} disabled={Boolean(browserActive)}>
              {profileIds.map(id => <NativeSelectOption value={id} key={id}>{profiles[id].title}</NativeSelectOption>)}
            </NativeSelect>
          </Field>}
          {section !== 'models' && !selected && <p className="text-muted-foreground">The local engine did not report this backend.</p>}
          {section === 'connections' && selected && <HuggingFaceConnection key={selectedId}
            backend={selected} account={accounts[selectedId]} disabled={disabled}
            browserSession={browserLogin?.backend === selectedId && browserActive ? browserLogin.session : null}
            cancelling={pending} onStartBrowser={() => startBrowserLogin(selectedId)} onCancelBrowser={cancelBrowserLogin}
            onCheck={token => verifyConnection(selectedId, token)}
            onDisconnect={() => openRemoval(selectedId, 'huggingface')} />}
          {section === 'storage' && selected && <ModelStorage key={selectedId} api={api} backend={selected} id={selectedId}
            disabled={disabled}
            onDownload={() => onRuntimeAction(selectedId, 'download')} onConvert={() => onRuntimeAction(selectedId, 'convert')}
            onSettings={onVisit}
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
  not_connected: 'Sign in to Hugging Face to check DINOv3 file access.',
  access_denied: 'DINOv3 access denied. Request access on the model page with the same account, then return here.',
  unavailable: 'Could not verify DINOv3 access. Select Refresh status to retry.',
};

const accountMessages: Record<AccountStatus, string> = {
  checking: 'Checking Hugging Face sign-in…',
  connected: 'Hugging Face account verified.',
  not_connected: 'No token is saved for this backend. Sign in or connect a token.',
  invalid_token: 'Hugging Face did not accept this token. Sign in again or use a different token.',
};

function browserLoginIsActive(session: HuggingFaceBrowserLogin) {
  return session.state === 'preparing' || session.state === 'awaiting_authorization';
}

function HuggingFaceConnection({ backend, account, disabled, browserSession, cancelling, onStartBrowser, onCancelBrowser, onCheck, onDisconnect }: {
  backend: Backend; account?: AccountStatus; disabled: boolean;
  browserSession: HuggingFaceBrowserLogin | null; cancelling: boolean;
  onStartBrowser: () => void; onCancelBrowser: () => void;
  onCheck: (token?: string) => void; onDisconnect: () => void;
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
      <div><h2 className="font-heading text-base font-medium">Hugging Face</h2><p className="text-muted-foreground">Credentials are stored locally and separately for each backend.</p></div>
      <Badge variant="outline">{account === 'connected' ? 'Signed in' : local?.token_saved ? 'Token saved' : local?.credentials_managed === false ? 'Custom storage' : 'Not connected'}</Badge>
    </div>
    {canConnect ? <>
      <div className="grid gap-3">
        <p className="text-muted-foreground">Sign in through Hugging Face without creating or copying a token. The first sign-in prepares a private Hugging Face CLI.</p>
        {browserSession?.state === 'preparing' && <p role="status">Preparing browser sign-in…</p>}
        {browserSession?.state === 'awaiting_authorization' && (
          <div className="grid justify-items-start gap-2 rounded-md border bg-background p-3">
            <p>Enter this one-time code on Hugging Face:</p>
            <code className="text-lg font-semibold tracking-widest select-all">{browserSession.user_code}</code>
            <p className="text-xs text-muted-foreground">The code is valid for up to {Math.ceil(browserSession.expires_in / 60)} minutes. MeshHarbor waits for approval.</p>
            <Button asChild><a href="https://huggingface.co/oauth/device" target="_blank" rel="noreferrer">Open Hugging Face</a></Button>
          </div>
        )}
        <div className="flex flex-wrap gap-2">
          {browserSession
            ? <Button variant="outline" disabled={cancelling} onClick={onCancelBrowser}>Cancel sign-in</Button>
            : <Button disabled={disabled || local?.credentials_managed === false} onClick={onStartBrowser}>{local?.token_saved ? 'Sign in again in browser' : 'Sign in with browser'}</Button>}
          <Button type="button" variant="outline" disabled={disabled} onClick={() => onCheck()}>Check sign-in</Button>
        </div>
      </div>
      <Disclosure className="rounded-md border p-3" title="Use an access token instead">
        <form method="post" autoComplete="off" className="mt-3 grid justify-items-start gap-2" onSubmit={connectToken}>
          <p className="text-muted-foreground"><a className="text-primary underline underline-offset-4" href="https://huggingface.co/settings/tokens" target="_blank" rel="noreferrer">Create a Hugging Face token</a> or use an existing one.</p>
          <Label htmlFor="huggingface-token">Hugging Face access token</Label>
          <Input id="huggingface-token" name="token" type="password" placeholder="hf_…" maxLength={512} required disabled={disabled} autoComplete="off" className="max-w-md" />
          <Button type="submit" disabled={disabled}>{local?.token_saved ? 'Replace token' : 'Connect token'}</Button>
        </form>
      </Disclosure>
    </> : <p className="text-muted-foreground">{backend.installable ? 'Set up the Python environment in Models before connecting a token.' : 'This backend cannot verify a token on this computer.'}</p>}
    {account === 'checking' && <p role="status" className="text-muted-foreground">{accountMessages.checking}</p>}
    {local?.disconnect_available && <div className="border-t pt-3"><Button variant="destructive" disabled={disabled} onClick={onDisconnect}>Disconnect locally</Button></div>}
    {local?.credentials_managed === false && <p className="text-muted-foreground">Browser sign-in and local disconnect require the default managed credential directory. For this profile, use an access token instead.</p>}
  </section>;
}

function ModelStorage({ api, backend, id, disabled, onDownload, onConvert, onSettings, onRemove }: {
  api: MeshHarborApi; backend: Backend; id: RuntimeBackendId; disabled: boolean;
  onDownload: () => void; onConvert: () => void; onSettings: VisitSettings; onRemove: () => void;
}) {
  const [consent, setConsent] = useState(false);
  const [access, setAccess] = useState<AccessStatus>('checking');
  const stage = runtimeSetupStage(backend);
  const local = backend.local_files;
  const needsConnection = local?.credentials_managed === true && !local.token_saved;
  const accessStatus = needsConnection ? 'not_connected' : access;
  const verificationRequired = accessStatus !== 'verified';
  const cache = backend.pixal3d_cache;
  useEffect(() => {
    if (stage !== 'models' || !backend.downloads_enabled || disabled || needsConnection) return;
    const controller = new AbortController();
    let checking = false;
    async function check() {
      if (checking || controller.signal.aborted) return;
      checking = true;
      setAccess('checking');
      const result = await api.checkModelAccess(id, controller.signal).catch(() => 'unavailable' as const);
      if (!controller.signal.aborted) setAccess(result);
      checking = false;
    }
    const initialCheck = window.setTimeout(() => void check(), 0);
    const onFocus = () => void check();
    window.addEventListener('focus', onFocus);
    return () => {
      window.clearTimeout(initialCheck);
      window.removeEventListener('focus', onFocus);
      controller.abort();
    };
  }, [api, backend, disabled, id, needsConnection, stage]);

  return <section className="grid gap-4 rounded-lg border bg-card p-4 sm:p-5" aria-label="Model files">
    <div className="flex flex-wrap items-start justify-between gap-2">
      <div><h2 className="font-heading text-base font-medium">Pixal3D · {profiles[id].title}</h2><p className="text-muted-foreground">Up to {profiles[id].download} without cached files</p></div>
      <Badge variant={backend.ready ? 'secondary' : 'outline'}>{backend.ready ? 'Installed' : backend.models_downloaded ? 'Files present' : local?.remove_models_available ? 'Cached files present' : 'Not installed'}</Badge>
    </div>
    {stage === 'unsupported' && <p className="text-muted-foreground">This backend is not supported on this computer.</p>}
    {stage === 'environment' && <div className="flex flex-wrap items-center gap-2"><p className="text-muted-foreground">Set up the Python environment first.</p><Button variant="outline" onClick={() => onSettings('models')}>View model setup</Button></div>}
    {stage === 'models' && <div className="grid justify-items-start gap-3">
      {cache && <p className="text-muted-foreground">
        {cache.local_checkpoints}/{cache.total_checkpoints} Pixal3D checkpoints in this backend’s cache.
        {cache.reusable_checkpoints > 0 && ` ${cache.reusable_checkpoints} more can be reused from your existing Hugging Face cache.`}
        {' DINOv3 and NAF are handled separately.'}
      </p>}
      <div className="grid gap-2">
        <p className="text-muted-foreground"><a className="text-primary underline underline-offset-4" href="https://huggingface.co/facebook/dinov3-vitl16-pretrain-lvd1689m" target="_blank" rel="noreferrer">Review terms and request DINOv3 access</a> on Hugging Face. Studio checks file access automatically.</p>
        {verificationRequired && accessStatus !== 'checking' && <Button className="justify-self-start" variant="outline" onClick={() => onSettings('connections', id)}>Manage Hugging Face connection</Button>}
      </div>
      <div role="status" className="text-muted-foreground">
        {accessStatus === 'verified' ? <Badge variant="secondary">{accessMessages.verified}</Badge> : accessMessages[accessStatus]}
      </div>
      <div className="flex items-start gap-2">
        <Checkbox id="model-download-consent" checked={consent}
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
