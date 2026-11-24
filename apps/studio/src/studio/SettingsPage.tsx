import { ArrowLeftIcon, ArrowsClockwiseIcon, CaretRightIcon, CopyIcon, CubeIcon, KeyIcon, ListChecksIcon, PaintBrushIcon, TrashIcon } from '@phosphor-icons/react';
import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Checkbox } from '@/components/ui/checkbox';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { Empty, EmptyDescription, EmptyHeader } from '@/components/ui/empty';
import { Field, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { ItemGroup } from '@/components/ui/item';
import { Label } from '@/components/ui/label';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { Sidebar, SidebarMenu, SidebarMenuButton, SidebarMenuItem, SidebarProvider } from '@/components/ui/sidebar';
import { Disclosure } from './Disclosure';
import { HelpTip } from './HelpTip';
import { JobListItem } from './JobListItem';
import type { StudioActivity } from './JobsDashboard';
import { NotificationHistory } from './NotificationHistory';
import { notify } from './notify';
import type { MeshHarborApi } from './api';
import { mergeJobRecords } from './job-activity';
import type { HuggingFaceAccess, HuggingFaceBrowserLogin, HuggingFaceConnectionStatus, Job, RuntimeBackendId, RuntimeInfo } from './types';
import { runtimeSetupStage } from './workbench-state';
import type { StudioTheme } from './theme';

const sections = {
  models: { title: 'Models', icon: CubeIcon, description: 'Install models and manage local files for generation.' },
  connections: { title: 'Connections', icon: KeyIcon, description: 'Manage sign-in.' },
  preferences: { title: 'Preferences', icon: PaintBrushIcon, description: 'Customize Studio.' },
};

export type SettingsSection = keyof typeof sections;
export type RuntimeAction = 'setup' | 'download' | 'convert' | 'multiview' | 'install' | 'install-gated';

const sectionIds: SettingsSection[] = ['models', 'connections', 'preferences'];
const accelerators = {
  mlx: { title: 'Apple MLX', platform: 'macOS · Apple Silicon', download: '24 GB' },
  cuda: { title: 'NVIDIA CUDA', platform: 'Linux x86-64 · NVIDIA GPU', download: '30 GB' },
};
const accelerator = (id: string) => id.endsWith('-cuda') ? 'cuda' : 'mlx';
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
  busy: boolean;
  theme: StudioTheme;
  jobs: Job[];
  operationLabel: (operation: string) => string;
  onJobDetails: (job: Job, trigger: HTMLButtonElement) => void;
  onVisit: VisitSettings;
  onClose: () => void;
  onOpenDashboard: () => void;
  onActivityChange: (activity: StudioActivity | null) => void;
  onReconnect: () => void;
  onRefreshJobs: () => Promise<boolean>;
  onThemeChange: (theme: StudioTheme) => void;
  onRuntimeChange: (runtime: RuntimeInfo) => void;
  onRuntimeAction: (backend: RuntimeBackendId, action: RuntimeAction) => void;
}

export function SettingsPage({ api, section, backendId, runtime, connection, busy, theme, jobs, operationLabel, onJobDetails, onVisit, onClose, onOpenDashboard, onActivityChange, onReconnect, onRefreshJobs, onThemeChange, onRuntimeChange, onRuntimeAction }: Props) {
  const heading = useRef<HTMLHeadingElement>(null);
  const removalTrigger = useRef<HTMLButtonElement | null>(null);
  const [accounts, setAccounts] = useState<Partial<Record<RuntimeBackendId, AccountStatus>>>({});
  const [pending, setPending] = useState(false);
  const [removal, setRemoval] = useState<{ backend: RuntimeBackendId; scope: RemovalScope; label?: string } | null>(null);
  const [browserLogin, setBrowserLogin] = useState<{ backend: RuntimeBackendId; session: HuggingFaceBrowserLogin } | null>(null);
  const platformId: RuntimeBackendId = runtime?.platform === 'linux' ? 'pixal3d-cuda' : 'pixal3d-mlx';
  const connectionId = section === 'connections' && backendId?.startsWith('pixal3d-') ? backendId : platformId;
  const connectionBackend = runtime?.model_backends.find(backend => backend.id === connectionId);
  const browserBackend = browserLogin?.backend;
  const browserSessionId = browserLogin?.session.id;
  const browserActive = browserLogin && browserLoginIsActive(browserLogin.session);
  const disabled = pending || busy || Boolean(browserActive);
  useEffect(() => {
    if (section === 'models' && backendId) {
      const cardHeading = document.getElementById(`runtime-${backendId}`);
      if (cardHeading) {
        cardHeading.focus();
        return;
      }
    }
    if (section) heading.current?.focus();
    else setRemoval(null);
  }, [section, backendId]);
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
            notify('success', 'Signed in to Hugging Face.');
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
      if (scope === 'model-files') onActivityChange({
        title: 'Remove model files',
        stage: `Removing local weights for ${removal.label}`,
        startedAt: Date.now(),
      });
      if (scope === 'model-files') notify('info', 'Removing local model files.');
      try {
        onRuntimeChange(await api.removeRuntimeFiles(backend, scope));
        if (scope === 'huggingface') {
          setAccounts(current => ({ ...current, [backend]: 'not_connected' }));
        }
        notify('success', scope === 'huggingface'
          ? 'Hugging Face disconnected on this computer.'
          : `Model files removed for ${removal.label}.`);
      } finally {
        if (scope === 'model-files') onActivityChange(null);
        setRemoval(null);
      }
    });
  }

  function openRemoval(backend: RuntimeBackendId, scope: RemovalScope, trigger: HTMLButtonElement, label?: string) {
    removalTrigger.current = trigger;
    setRemoval({ backend, scope, label });
  }

  if (!section) return null;
  const sectionInfo = sections[section];

  return <div className="min-h-dvh bg-background text-foreground">
    <header className="border-b bg-background">
      <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-3 px-4 py-2 sm:px-6">
        <Button variant="ghost" size="sm" onClick={onClose}><ArrowLeftIcon aria-hidden="true" /> Workbench</Button>
        <span className="border-l pl-3 font-heading text-sm font-medium">Settings</span>
        <div className="ml-auto flex items-center gap-1">
          <Button variant="ghost" size="sm" onClick={onOpenDashboard}><ListChecksIcon aria-hidden="true" /> Dashboard</Button>
          <NotificationHistory />
        </div>
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
        </Card> : connection !== 'connected' ? <Alert role="status" className="justify-items-start gap-2 p-4">
          <AlertTitle><h2>{connection === 'connecting' ? 'Connecting to the local engine' : 'Local engine is offline'}</h2></AlertTitle>
          {connection === 'disconnected' && <AlertDescription>Start <code>meshharbor serve</code> to connect.</AlertDescription>}
          <Button disabled={connection === 'connecting'} onClick={onReconnect}>Retry connection</Button>
        </Alert> : <>
          {section === 'models' && (runtime?.model_catalog ?? []).map(model => <Card key={model.id}>
            <CardHeader>
              <CardTitle className="flex flex-wrap items-center gap-2 text-lg">
                <h2>{model.title}</h2>
                {(['image', 'text'] as const).filter(mode => runtime?.model_backends.some(backend =>
                  model.backend_ids.includes(backend.id) && (backend.generation?.input_modes ?? ['image']).includes(mode)
                )).map(mode => <Badge key={mode} variant="outline">{mode === 'image' ? 'Image to 3D' : 'Text to 3D'}</Badge>)}
                {model.status === 'experimental_opt_in' && <Badge variant="outline">Experimental</Badge>}
              </CardTitle>
              <CardDescription>Output: {model.output === 'geometry_glb' ? 'geometry-only GLB' : 'textured GLB'}</CardDescription>
            </CardHeader>
            <CardContent className="grid gap-3 lg:grid-cols-2">
              {(['mlx', 'cuda'] as const).map(kind => {
                const id = model.backend_ids.find(candidate => candidate.endsWith(`-${kind}`)) as RuntimeBackendId | undefined;
                const backend = runtime?.model_backends.find(item => item.id === id);
                if (!id || !backend) return null;
                const label = `${model.title} · ${accelerators[kind].title}`;
                const modelPaths = backend.model_paths ?? (backend.runtime_root ? [backend.runtime_root] : []);
                return <Card key={id} role="article" aria-label={label} size="sm" className="min-w-0 bg-background">
                  <CardHeader>
                    <CardTitle className="min-w-0"><h3 id={`runtime-${id}`} tabIndex={-1} className="outline-none">{accelerators[kind].title}</h3></CardTitle>
                    <CardDescription>{accelerators[kind].platform}</CardDescription>
                    <CardAction><Badge variant={backend.ready ? 'secondary' : 'outline'}>{profileStatus(backend)}</Badge></CardAction>
                  </CardHeader>
                  <CardContent className="grid gap-3">
                    {modelPaths.length > 0 && <ModelFileLocations paths={modelPaths} />}
                    {backend.managed_install
                      ? <CatalogInstall label={label} backend={backend} disabled={disabled} account={connectionBackendFor(id, runtime)}
                          onConnections={() => onVisit('connections', kind === 'cuda' ? 'pixal3d-cuda' : 'pixal3d-mlx')}
                          onInstall={() => onRuntimeAction(id, backend.gated_model_access_required ? 'install-gated' : 'install')} />
                      : <PixalModelFiles label={label} api={api} backend={backend} id={id} disabled={disabled}
                          onAction={action => onRuntimeAction(id, action)} onSettings={onVisit} />}
                  </CardContent>
                  <CardFooter className="mt-auto grid gap-2 border-t">
                    <RuntimeLogs api={api} id={id} jobs={jobs} operationLabel={operationLabel} onJobDetails={onJobDetails}
                      action={backend.local_files?.remove_models_available && <Button variant="destructive" size="sm" onClick={event => openRemoval(id, 'model-files', event.currentTarget, label)} disabled={disabled}>
                        <TrashIcon aria-hidden="true" /> Delete model files
                      </Button>} />
                  </CardFooter>
                </Card>;
              })}
            </CardContent>
          </Card>)}
          {section === 'models' && !runtime?.model_catalog?.length && <Empty><EmptyHeader><EmptyDescription>No generation models reported by the local engine.</EmptyDescription></EmptyHeader></Empty>}
          {section === 'connections' && (connectionBackend ? <HuggingFaceConnection key={connectionId}
            backend={connectionBackend} account={accounts[connectionId]} disabled={disabled}
            browserSession={browserLogin?.backend === connectionId && browserActive ? browserLogin.session : null}
            cancelling={pending} onStartBrowser={() => startBrowserLogin(connectionId)} onCancelBrowser={cancelBrowserLogin}
            onCheck={token => verifyConnection(connectionId, token)}
            onDisconnect={trigger => openRemoval(connectionId, 'huggingface', trigger)} />
            : <Empty><EmptyHeader><EmptyDescription>Hugging Face sign-in is not available on this computer.</EmptyDescription></EmptyHeader></Empty>)}
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
            ? 'Remove saved local credentials. Your Hugging Face token and downloaded models stay intact.'
            : `Remove managed weights and caches for ${removal.label}. Your environment, sign-in, and assets stay intact.`)}</AlertDialogDescription>
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
  if (!backend.managed_install && !backend.environment_ready) return 'Setup required';
  if (backend.models_downloaded) return 'Files present';
  return backend.downloads_enabled ? 'Available to download' : 'Unavailable';
}

function connectionBackendFor(id: RuntimeBackendId, runtime: RuntimeInfo | null) {
  return runtime?.model_backends.find(backend => backend.id === `pixal3d-${accelerator(id)}`);
}

function ModelFileLocations({ paths }: { paths: string[] }) {
  async function copyPaths() {
    try {
      await navigator.clipboard.writeText(paths.join('\n'));
      notify('success', 'Model file locations copied.');
    } catch {
      notify('error', 'Could not copy model file locations.');
    }
  }

  return <dl className="grid min-w-0 gap-1">
    <dt className="font-medium">Model file locations</dt>
    <dd className="flex min-w-0 items-center gap-2">
      <code className="min-w-0 flex-1 truncate text-muted-foreground">{paths.join(' · ')}</code>
      <Button type="button" variant="ghost" size="icon-sm" aria-label="Copy model file locations" title="Copy model file locations"
        onClick={() => void copyPaths()}><CopyIcon aria-hidden="true" /></Button>
    </dd>
  </dl>;
}

function RuntimeLogs({ api, id, jobs, operationLabel, onJobDetails, action }: {
  api: MeshHarborApi; id: RuntimeBackendId; jobs: Job[];
  operationLabel: Props['operationLabel']; onJobDetails: Props['onJobDetails']; action: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [saved, setSaved] = useState<Job[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    void api.runtimeJobs(id, controller.signal).then(records => {
      if (!controller.signal.aborted) {
        setSaved(records);
        setFailed(false);
      }
    }).catch(() => { if (!controller.signal.aborted) setFailed(true); });
    return () => controller.abort();
  }, [api, id, open, attempt]);
  const records = mergeJobRecords(jobs.filter(job =>
    job.operation.startsWith('runtime.') && job.parameters?.backend_id === id), saved ?? []).slice(0, 6);
  return <Collapsible open={open} onOpenChange={setOpen} className="min-w-0">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <CollapsibleTrigger asChild>
        <Button variant="outline" size="sm"><CaretRightIcon className="transition-transform group-aria-expanded/button:rotate-90 motion-reduce:transition-none" aria-hidden="true" /> Recent logs</Button>
      </CollapsibleTrigger>
      {action}
    </div>
    <CollapsibleContent className="min-w-0 pt-2">
      {records.length > 0 && <ItemGroup>
        {records.map(job => <JobListItem key={job.id} job={job} title={operationLabel(job.operation)} onDetails={onJobDetails} />)}
      </ItemGroup>}
      {failed ? <p className="text-muted-foreground">Could not load recent logs. <button type="button" className="underline" onClick={() => { setFailed(false); setAttempt(current => current + 1); }}>Retry</button></p>
        : !saved ? <p role="status" className="text-muted-foreground">Loading recent logs…</p>
          : records.length === 0 && <p className="text-muted-foreground">No install or setup logs for this backend.</p>}
    </CollapsibleContent>
  </Collapsible>;
}

function ModelInstallDialog({ label, action, description, consentLabel, disabled, blocked = false, onOpenChange, onConfirm, children }: {
  label: string; action: string; description: string; consentLabel: string;
  disabled: boolean; blocked?: boolean; onOpenChange?: (open: boolean) => void;
  onConfirm: () => void; children?: ReactNode;
}) {
  const consentId = useId();
  const [consent, setConsent] = useState(false);
  return <AlertDialog onOpenChange={open => {
    if (!open) setConsent(false);
    onOpenChange?.(open);
  }}>
    <AlertDialogTrigger asChild><Button className="justify-self-start" disabled={disabled}>{action}</Button></AlertDialogTrigger>
    <AlertDialogContent className="max-h-[calc(100dvh-2rem)] w-[calc(100%-2rem)] grid-rows-[auto_minmax(0,1fr)_auto] data-[size=default]:sm:max-w-md">
      <AlertDialogHeader>
        <AlertDialogTitle>{action}?</AlertDialogTitle>
        <AlertDialogDescription><span className="block font-medium text-foreground">{label}</span>{description}</AlertDialogDescription>
      </AlertDialogHeader>
      <div className="grid min-h-0 justify-items-start gap-3 overflow-y-auto overscroll-contain px-1 py-1 text-xs/relaxed">
        {children}
        <div className="flex items-start gap-2">
          <Checkbox id={consentId} checked={consent} disabled={disabled || blocked} onCheckedChange={value => setConsent(value === true)} />
          <Label htmlFor={consentId} className="leading-relaxed">{consentLabel}</Label>
        </div>
      </div>
      <AlertDialogFooter>
        <AlertDialogCancel>Cancel</AlertDialogCancel>
        <AlertDialogAction disabled={disabled || blocked || !consent} onClick={onConfirm}>{action}</AlertDialogAction>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>;
}

const dinoAccessUrl = 'https://huggingface.co/facebook/dinov3-vitl16-pretrain-lvd1689m';

function DinoAccessLink() {
  return <a className="text-primary underline underline-offset-4" href={dinoAccessUrl} target="_blank" rel="noreferrer">Review terms and request DINOv3 access</a>;
}

function CatalogInstall({ label, backend, account, disabled, onConnections, onInstall }: {
  label: string; backend: Backend; account?: Backend; disabled: boolean; onConnections: () => void; onInstall: () => void;
}) {
  if (!backend.downloads_enabled) return null;
  const gated = backend.gated_model_access_required;
  const needsSignIn = gated && !account?.local_files?.token_saved && account?.local_files?.credentials_managed !== false;
  return <ModelInstallDialog label={label} action="Install model"
    description="MeshHarbor installs a private Python environment and downloads the model weights for local generation."
    consentLabel={gated ? 'I have DINOv3 access, reviewed the terms, and approve installation and download.' : 'I reviewed the terms and approve installation and download.'}
    disabled={disabled} blocked={needsSignIn} onConfirm={onInstall}>
    {backend.ready && !backend.managed_ready && <p className="text-muted-foreground">Using existing files. A managed install keeps them intact.</p>}
    {backend.license_notice && <p className="text-muted-foreground">{backend.license_notice}</p>}
    {backend.source && <a className="text-primary underline underline-offset-4" href={`${backend.source.repository}/tree/${backend.source.revision}`} target="_blank" rel="noreferrer">Review upstream source and terms</a>}
    {gated && <>
      <DinoAccessLink />
      {needsSignIn && <Button variant="outline" onClick={onConnections}>Manage Hugging Face connection</Button>}
    </>}
  </ModelInstallDialog>;
}

const accessMessages: Record<AccessStatus, string> = {
  checking: 'Checking DINOv3 file access…',
  verified: 'DINOv3 file access verified.',
  not_connected: 'Sign in to Hugging Face to check DINOv3 file access.',
  access_denied: 'Access required',
  unavailable: 'Access not verified',
};

const accountMessages: Record<AccountStatus, string> = {
  checking: 'Checking Hugging Face sign-in…',
  connected: 'Hugging Face account verified.',
  not_connected: 'No local token is saved. Sign in or connect a token.',
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
  const canConnect = backend.installable;
  const canVerify = backend.account_tools_ready ?? backend.environment_ready;
  function connectToken(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const token = new FormData(event.currentTarget).get('token');
    event.currentTarget.reset();
    if (typeof token === 'string') onCheck(token);
  }
  return <Card role="region" aria-label="Hugging Face connection">
    <CardHeader>
      <CardTitle className="flex min-w-0 items-center gap-1 text-base"><h2>Hugging Face</h2>
        <HelpTip label="About Hugging Face sign-in">
          Credentials stay in the local runtime. The first browser sign-in prepares a private Hugging Face CLI.
          {local?.credentials_managed === false && ' Custom credential storage requires an access token instead of browser sign-in.'}
        </HelpTip>
      </CardTitle>
      <CardAction><Badge variant="outline">{account === 'connected' ? 'Signed in' : local?.token_saved ? 'Token saved' : local?.credentials_managed === false ? 'Custom storage' : 'Not connected'}</Badge></CardAction>
    </CardHeader>
    <CardContent className="grid gap-4">
    {canConnect ? <>
      <div className="grid gap-3">
        {browserSession?.state === 'preparing' && <p role="status">Preparing browser sign-in…</p>}
        {browserSession?.state === 'awaiting_authorization' && (
          <Card size="sm" className="bg-background"><CardContent className="grid justify-items-start gap-2">
            <p>Enter this one-time code on Hugging Face:</p>
            <code className="text-lg font-semibold tracking-widest select-all">{browserSession.user_code}</code>
            <p className="text-xs text-muted-foreground">Valid for up to {Math.ceil(browserSession.expires_in / 60)} min.</p>
            <Button asChild><a href="https://huggingface.co/oauth/device" target="_blank" rel="noreferrer">Open Hugging Face</a></Button>
          </CardContent></Card>
        )}
        <div className="flex flex-wrap gap-2">
          {browserSession
            ? <Button variant="outline" disabled={cancelling} onClick={onCancelBrowser}>Cancel sign-in</Button>
            : <Button disabled={disabled || local?.credentials_managed === false} onClick={onStartBrowser}>{local?.token_saved ? 'Sign in again in browser' : 'Sign in with browser'}</Button>}
          <Button type="button" variant="outline" disabled={disabled || !canVerify} onClick={() => onCheck()}>Check sign-in</Button>
        </div>
      </div>
      {canVerify && <Disclosure className="rounded-md border p-3" title="Use an access token instead">
        <form method="post" autoComplete="off" className="mt-3 grid justify-items-start gap-2" onSubmit={connectToken}>
          <a className="text-primary underline underline-offset-4" href="https://huggingface.co/settings/tokens" target="_blank" rel="noreferrer">Create a Hugging Face token</a>
          <Label htmlFor="huggingface-token">Hugging Face access token</Label>
          <Input id="huggingface-token" name="token" type="password" placeholder="hf_…" maxLength={512} required disabled={disabled} autoComplete="off" className="max-w-md" />
          <Button type="submit" disabled={disabled}>{local?.token_saved ? 'Replace token' : 'Connect token'}</Button>
        </form>
      </Disclosure>}
    </> : <p className="text-muted-foreground">Hugging Face sign-in is not available on this computer.</p>}
    {account === 'checking' && <p role="status" className="text-muted-foreground">{accountMessages.checking}</p>}
    {local?.disconnect_available && <div className="border-t pt-3"><Button variant="destructive" disabled={disabled} onClick={event => onDisconnect(event.currentTarget)}>Disconnect locally</Button></div>}
    </CardContent>
  </Card>;
}

function PixalModelFiles({ label, api, backend, id, disabled, onAction, onSettings }: {
  label: string; api: MeshHarborApi; backend: Backend; id: RuntimeBackendId; disabled: boolean;
  onAction: (action: RuntimeAction) => void; onSettings: VisitSettings;
}) {
  const stage = runtimeSetupStage(backend);
  if (stage === 'environment') return <Button className="justify-self-start" disabled={disabled} onClick={() => onAction('setup')}>Set up {id === 'pixal3d-cuda' ? 'CUDA' : 'Python'} environment</Button>;
  if (stage === 'models' && backend.downloads_enabled) return <PixalDownloadDialog label={label} api={api} backend={backend} id={id} disabled={disabled}
    onConnections={() => onSettings('connections', id)} onDownload={() => onAction('download')} />;
  if (stage === 'conversion') return <Button className="justify-self-start" disabled={disabled || !backend.conversion_enabled} onClick={() => onAction('convert')}>Finish NAF conversion</Button>;
  if (stage === 'incomplete') return <div className="flex items-center gap-1 text-muted-foreground">
    Runtime not ready <HelpTip label="About runtime checks">Files are present. Run <code>meshharbor doctor</code> for diagnostics.</HelpTip>
  </div>;
  if (stage !== 'ready' || !backend.generation?.view_modes?.some(mode => mode !== 'single')) return null;
  if (backend.multiview_ready) return <Badge variant="outline">Multiview installed</Badge>;
  return <ModelInstallDialog label={label} action="Install multiview weights"
    description="Adds support for multiple reference views. This separate download can be large; MeshHarbor reuses compatible cached files."
    consentLabel="I approve installing the additional model files." disabled={disabled} onConfirm={() => onAction('multiview')} />;
}

function PixalDownloadDialog({ label, api, backend, id, disabled, onConnections, onDownload }: {
  label: string; api: MeshHarborApi; backend: Backend; id: RuntimeBackendId; disabled: boolean;
  onConnections: () => void; onDownload: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [access, setAccess] = useState<AccessStatus>('checking');
  const local = backend.local_files;
  const needsConnection = local?.credentials_managed === true && !local.token_saved;
  const accessStatus = needsConnection ? 'not_connected' : access;
  const verificationRequired = accessStatus !== 'verified';
  const cache = backend.pixal3d_cache;
  useEffect(() => {
    if (!open || disabled || needsConnection) return;
    const controller = new AbortController();
    let checking = false;
    let previous: AccessStatus | undefined;
    async function check() {
      if (checking || controller.signal.aborted) return;
      checking = true;
      setAccess('checking');
      const result = await api.checkModelAccess(id, controller.signal).catch(() => 'unavailable' as const);
      if (!controller.signal.aborted) {
        setAccess(result);
        if (previous !== result && result === 'access_denied') {
          notify('warning', 'DINOv3 access denied.', 'Request access on Hugging Face with the same account.');
        } else if (previous !== result && result === 'unavailable') {
          notify('error', 'Could not verify DINOv3 access.', 'Refresh status to retry.');
        }
        previous = result;
      }
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
  }, [api, backend, disabled, id, needsConnection, open]);

  return <ModelInstallDialog label={label} action="Download model files"
    description={`Up to ${accelerators[accelerator(id)].download} without cached files. DINOv3 access is required.`}
    consentLabel="I reviewed the model terms and approve this download." disabled={disabled} blocked={verificationRequired}
    onOpenChange={next => { if (next) setAccess('checking'); setOpen(next); }} onConfirm={onDownload}>
    {cache && <p className="text-muted-foreground">
      {cache.local_checkpoints}/{cache.total_checkpoints} checkpoints cached.
      {cache.reusable_checkpoints > 0 && ` ${cache.reusable_checkpoints} more can be reused from the Hugging Face cache.`}
    </p>}
    <DinoAccessLink />
    {verificationRequired && accessStatus !== 'checking' && <Button variant="outline" onClick={onConnections}>Manage Hugging Face connection</Button>}
    <div role="status" className="text-muted-foreground">
      {accessStatus === 'verified' ? <Badge variant="secondary">{accessMessages.verified}</Badge> : accessMessages[accessStatus]}
    </div>
  </ModelInstallDialog>;
}
