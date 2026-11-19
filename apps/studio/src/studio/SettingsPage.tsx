import { ArrowLeftIcon, ArrowsClockwiseIcon, CubeIcon, HardDrivesIcon, KeyIcon, PaintBrushIcon } from '@phosphor-icons/react';
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Checkbox } from '@/components/ui/checkbox';
import { Empty, EmptyDescription, EmptyHeader } from '@/components/ui/empty';
import { Field, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { ItemGroup } from '@/components/ui/item';
import { Label } from '@/components/ui/label';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { Sidebar, SidebarMenu, SidebarMenuButton, SidebarMenuItem, SidebarProvider } from '@/components/ui/sidebar';
import { Disclosure } from './Disclosure';
import { JobListItem } from './JobListItem';
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
export type RuntimeAction = 'setup' | 'download' | 'convert' | 'multiview';

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
  const removalTrigger = useRef<HTMLButtonElement | null>(null);
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
    if (!removal || disabled) return;
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

  function openRemoval(backend: RuntimeBackendId, scope: RemovalScope, trigger: HTMLButtonElement) {
    removalTrigger.current = trigger;
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
      <SidebarProvider className="contents" keyboardShortcut={false}>
        <Sidebar collapsible="none" className="h-auto w-full bg-transparent">
          <nav aria-label="Settings sections">
            <SidebarMenu className="flex-row flex-wrap md:flex-col">
              {sectionIds.map(id => {
                const { title, icon: Icon } = sections[id];
                return <SidebarMenuItem key={id}>
                  <SidebarMenuButton type="button" isActive={section === id}
                    aria-current={section === id ? 'page' : undefined} onClick={() => onVisit(id)}>
                    <Icon aria-hidden="true" />{title}
                  </SidebarMenuButton>
                </SidebarMenuItem>;
              })}
            </SidebarMenu>
          </nav>
        </Sidebar>
      </SidebarProvider>
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
        {section === 'preferences' ? <Card>
          <CardHeader><CardTitle className="text-base"><h2>Appearance</h2></CardTitle></CardHeader>
          <CardContent>
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
          </CardContent>
        </Card> : connection !== 'connected' ? <Alert role={connection === 'connecting' ? 'status' : 'alert'} className="justify-items-start gap-2 p-4">
          <AlertTitle><h2>{connection === 'connecting' ? 'Connecting to the local engine' : 'Local engine is offline'}</h2></AlertTitle>
          <AlertDescription className="grid gap-2">
            <p className="mb-0!">Start <code>meshharbor serve</code> to view and manage local models.</p>
            {connectionError && <p className="wrap-anywhere">{connectionError}</p>}
          </AlertDescription>
          <Button disabled={connection === 'connecting'} onClick={onReconnect}>Retry connection</Button>
        </Alert> : <>
          {section === 'models' && (runtime?.model_catalog ?? []).map(model => <Card key={model.id}>
            <CardHeader>
              <CardTitle className="flex flex-wrap items-center gap-2 text-lg">
                <h2>{model.title}</h2>
                <Badge variant="outline">Image to 3D</Badge>
                {model.status === 'experimental_opt_in' && <Badge variant="outline">Experimental</Badge>}
              </CardTitle>
              <CardDescription>Generate a textured GLB from a reference image. Once installed, generation runs locally.</CardDescription>
            </CardHeader>
            <CardContent className="grid gap-3 lg:grid-cols-2">
              {profileIds.filter(id => model.backend_ids.includes(id)).map(id => {
                const backend = runtime?.model_backends.find(item => item.id === id);
                if (!backend) return null;
                return <Card key={id} role="article" size="sm" className="bg-background">
                  <CardHeader>
                    <CardTitle className="min-w-0"><h3>{profiles[id].title}</h3></CardTitle>
                    <CardDescription>{profiles[id].platform}</CardDescription>
                    <CardAction><Badge variant={backend.ready ? 'secondary' : 'outline'}>{profileStatus(backend)}</Badge></CardAction>
                  </CardHeader>
                  <CardFooter className="flex-wrap gap-2">
                    {runtimeSetupStage(backend) === 'environment' && <Button onClick={() => onRuntimeAction(id, 'setup')} disabled={disabled}>Set up {id === 'pixal3d-cuda' ? 'CUDA' : 'Python'} environment</Button>}
                    {(backend.installable || backend.local_files?.remove_models_available) && <Button variant="outline" onClick={() => onVisit('storage', id)}>{backend.ready ? 'Manage files' : 'View downloads'}</Button>}
                  </CardFooter>
                </Card>;
              })}
            </CardContent>
          </Card>)}
          {section === 'models' && !runtime?.model_catalog?.length && <Empty><EmptyHeader><EmptyDescription>No generation models reported by the local engine.</EmptyDescription></EmptyHeader></Empty>}
          {section !== 'models' && <Field className="max-w-xs">
            <FieldLabel htmlFor="settings-backend">Backend</FieldLabel>
            <NativeSelect id="settings-backend" className="w-full" value={selectedId} onChange={event => {
              const choice = profileIds.find(id => id === event.target.value);
              if (choice) onProfileChange(choice);
            }} disabled={Boolean(browserActive)}>
              {profileIds.map(id => <NativeSelectOption value={id} key={id}>{profiles[id].title}</NativeSelectOption>)}
            </NativeSelect>
          </Field>}
          {section !== 'models' && !selected && <Empty><EmptyHeader><EmptyDescription>The local engine did not report this backend.</EmptyDescription></EmptyHeader></Empty>}
          {section === 'connections' && selected && <HuggingFaceConnection key={selectedId}
            backend={selected} account={accounts[selectedId]} disabled={disabled}
            browserSession={browserLogin?.backend === selectedId && browserActive ? browserLogin.session : null}
            cancelling={pending} onStartBrowser={() => startBrowserLogin(selectedId)} onCancelBrowser={cancelBrowserLogin}
            onCheck={token => verifyConnection(selectedId, token)}
            onDisconnect={trigger => openRemoval(selectedId, 'huggingface', trigger)} />}
          {section === 'storage' && selected && <ModelStorage key={selectedId} api={api} backend={selected} id={selectedId}
            disabled={disabled}
            onDownload={() => onRuntimeAction(selectedId, 'download')} onConvert={() => onRuntimeAction(selectedId, 'convert')}
            onMultiview={() => onRuntimeAction(selectedId, 'multiview')}
            onSettings={onVisit}
            onRemove={trigger => openRemoval(selectedId, 'model-files', trigger)} />}
          {section === 'storage' && <Card role="region" aria-labelledby="runtime-activity-heading">
            <CardHeader><CardTitle className="text-base"><h2 id="runtime-activity-heading">Recent activity</h2></CardTitle></CardHeader>
            <CardContent>
              {runtimeJobs.length ? <ItemGroup>
                {runtimeJobs.map(job => <JobListItem key={job.id} job={job} title={operationLabel(job.operation)} />)}
              </ItemGroup> : <Empty className="p-3"><EmptyHeader><EmptyDescription>No setup or download jobs yet.</EmptyDescription></EmptyHeader></Empty>}
            </CardContent>
          </Card>}
        </>}
      </main>
    </div>
    <AlertDialog open={removal !== null} onOpenChange={open => { if (!open && !pending) setRemoval(null); }}>
      <AlertDialogContent className="w-[calc(100%-2rem)]"
        onEscapeKeyDown={event => { if (pending) event.preventDefault(); }} onCloseAutoFocus={event => {
        event.preventDefault();
        const trigger = removalTrigger.current;
        if (trigger?.isConnected && trigger.getClientRects().length) trigger.focus();
        else heading.current?.focus();
      }}>
        <AlertDialogHeader>
          <AlertDialogTitle>{removal?.scope === 'huggingface' ? 'Disconnect Hugging Face?' : 'Remove model files?'}</AlertDialogTitle>
          <AlertDialogDescription>{removal && (removal.scope === 'huggingface'
            ? `Remove the locally saved Hugging Face credentials for ${profiles[removal.backend].title}. This does not revoke the token on Hugging Face or remove downloaded models.`
            : `Remove Pixal3D weights and transfer caches for ${profiles[removal.backend].title}. The Python environment, Hugging Face connection, and all Studio assets stay in place.`)}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
          <AlertDialogAction variant="destructive" disabled={disabled} onClick={event => {
            event.preventDefault();
            confirmRemoval();
          }}>{pending ? 'Removing…' : removal?.scope === 'huggingface' ? 'Disconnect' : 'Remove files'}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
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
  onCheck: (token?: string) => void; onDisconnect: (trigger: HTMLButtonElement) => void;
}) {
  const local = backend.local_files;
  const canConnect = backend.installable && backend.environment_ready;
  function connectToken(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const token = new FormData(event.currentTarget).get('token');
    event.currentTarget.reset();
    if (typeof token === 'string') onCheck(token);
  }
  return <Card role="region" aria-label="Hugging Face connection">
    <CardHeader>
      <CardTitle className="min-w-0 text-base"><h2>Hugging Face</h2></CardTitle>
      <CardDescription>Credentials are stored locally and separately for each backend.</CardDescription>
      <CardAction><Badge variant="outline">{account === 'connected' ? 'Signed in' : local?.token_saved ? 'Token saved' : local?.credentials_managed === false ? 'Custom storage' : 'Not connected'}</Badge></CardAction>
    </CardHeader>
    <CardContent className="grid gap-4">
    {canConnect ? <>
      <div className="grid gap-3">
        <p className="text-muted-foreground">Sign in through Hugging Face without creating or copying a token. The first sign-in prepares a private Hugging Face CLI.</p>
        {browserSession?.state === 'preparing' && <p role="status">Preparing browser sign-in…</p>}
        {browserSession?.state === 'awaiting_authorization' && (
          <Card size="sm" className="bg-background"><CardContent className="grid justify-items-start gap-2">
            <p>Enter this one-time code on Hugging Face:</p>
            <code className="text-lg font-semibold tracking-widest select-all">{browserSession.user_code}</code>
            <p className="text-xs text-muted-foreground">The code is valid for up to {Math.ceil(browserSession.expires_in / 60)} minutes. MeshHarbor waits for approval.</p>
            <Button asChild><a href="https://huggingface.co/oauth/device" target="_blank" rel="noreferrer">Open Hugging Face</a></Button>
          </CardContent></Card>
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
    {local?.disconnect_available && <div className="border-t pt-3"><Button variant="destructive" disabled={disabled} onClick={event => onDisconnect(event.currentTarget)}>Disconnect locally</Button></div>}
    {local?.credentials_managed === false && <p className="text-muted-foreground">Browser sign-in and local disconnect require the default managed credential directory. For this profile, use an access token instead.</p>}
    </CardContent>
  </Card>;
}

function ModelStorage({ api, backend, id, disabled, onDownload, onConvert, onMultiview, onSettings, onRemove }: {
  api: MeshHarborApi; backend: Backend; id: RuntimeBackendId; disabled: boolean;
  onDownload: () => void; onConvert: () => void; onMultiview: () => void; onSettings: VisitSettings; onRemove: (trigger: HTMLButtonElement) => void;
}) {
  const [consent, setConsent] = useState(false);
  const [multiviewConsent, setMultiviewConsent] = useState(false);
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

  return <Card role="region" aria-label="Model files">
    <CardHeader>
      <CardTitle className="min-w-0 text-base"><h2>Pixal3D · {profiles[id].title}</h2></CardTitle>
      <CardDescription>Up to {profiles[id].download} without cached files</CardDescription>
      <CardAction><Badge variant={backend.ready ? 'secondary' : 'outline'}>{backend.ready ? 'Installed' : backend.models_downloaded ? 'Files present' : local?.remove_models_available ? 'Cached files present' : 'Not installed'}</Badge></CardAction>
    </CardHeader>
    <CardContent className="grid gap-4">
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
    {stage === 'ready' && backend.generation?.view_modes?.some(mode => mode !== 'single') &&
      <section className="grid justify-items-start gap-2 border-t pt-3" aria-label="Multiview model files">
        <h3 className="font-medium">Multiview weights</h3>
        {backend.multiview_ready ? <p className="text-muted-foreground">Installed. Multiple reference views are available in Generate model.</p>
          : <>
            <p className="text-muted-foreground">Uses a separate model snapshot. This may transfer several large files; compatible cached files are reused.</p>
            <div className="flex items-start gap-2">
              <Checkbox id="multiview-consent" checked={multiviewConsent} disabled={disabled} onCheckedChange={value => setMultiviewConsent(value === true)} />
              <Label htmlFor="multiview-consent">I approve installing the additional model files.</Label>
            </div>
            <Button variant="outline" disabled={disabled || !multiviewConsent}
              onClick={() => { setMultiviewConsent(false); onMultiview(); }}>Install multiview weights</Button>
          </>}
      </section>}
    {local?.remove_models_available && <div className="grid justify-items-start gap-2 border-t pt-3">
      <p className="text-muted-foreground">Removing model files does not remove the Python environment, saved credentials, or assets.</p>
      <Button variant="destructive" disabled={disabled} onClick={event => onRemove(event.currentTarget)}>{backend.models_downloaded ? 'Uninstall model files' : 'Clear cached model files'}</Button>
    </div>}
    {local?.model_files_managed === false && <p className="text-muted-foreground">Studio cannot remove files from custom or linked model directories.</p>}
    </CardContent>
  </Card>;
}
