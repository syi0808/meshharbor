import { ArrowDownIcon, ArrowLeftIcon, ArrowUpIcon, ArrowUpRightIcon, ArrowsClockwiseIcon, CubeIcon, ListChecksIcon } from '@phosphor-icons/react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty';
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import { isActiveJob, isGenerationOperation, isJobCancellable, type MeshHarborApi } from './api';
import { HelpTip } from './HelpTip';
import { JOB_PAGE_SIZE, formatDuration, generationJobName, jobStage, jobTiming, lastGenerationStage, mergeJobRecords } from './job-activity';
import { JobListItem } from './JobListItem';
import { NotificationHistory } from './NotificationHistory';
import { notify } from './notify';
import type { Job } from './types';

export interface StudioActivity {
  title: string;
  stage: string;
  startedAt: number;
}

function useJobClock(ticking: boolean) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!ticking) return;
    const update = () => setNow(Date.now());
    update();
    const timer = window.setInterval(update, 1000);
    return () => window.clearInterval(timer);
  }, [ticking]);
  return now;
}

export function PendingGenerationCards({ jobs, sources, selectedId, onSelect }: {
  jobs: Job[];
  sources: ReadonlyMap<string, string>;
  selectedId: string | null;
  onSelect: (job: Job) => void;
}) {
  if (!jobs.length) return null;
  return <section aria-label="Models in progress" className="grid min-w-0 gap-2">
    <h3 className="flex items-center gap-2 text-xs font-medium">
      In progress <Badge variant="secondary">{jobs.length}</Badge>
    </h3>
    <div className="grid min-w-0 grid-cols-2 gap-2 @min-[380px]:grid-cols-3 max-[330px]:grid-cols-1">
      {jobs.map(job => {
        const name = generationJobName(job, sources);
        return <button key={job.id} type="button" aria-pressed={job.id === selectedId} onClick={() => onSelect(job)}
          aria-label={`Select pending model ${name}`}
          className="min-w-0 overflow-hidden rounded-xl border bg-card text-left text-card-foreground outline-none hover:border-primary/70 focus-visible:ring-2 focus-visible:ring-ring aria-pressed:border-primary aria-pressed:ring-1 aria-pressed:ring-primary">
          <span className="relative grid aspect-square w-full place-items-center overflow-hidden border-b bg-muted/60">
            <CubeIcon className="size-10 text-primary/60" weight="duotone" aria-hidden="true" />
            <Badge className="absolute inset-x-1.5 bottom-1.5 mx-auto max-w-full" variant="secondary">
              <Spinner aria-hidden="true" /> {job.state === 'queued' ? 'Queued' : job.state === 'cancelling' ? 'Stopping' : 'Generating'}
            </Badge>
          </span>
          <span className="grid min-w-0 gap-1 p-2">
            <span className="truncate font-medium" title={name}>{name}</span>
            <span className="truncate text-muted-foreground" title={jobStage(job)}>{jobStage(job)}</span>
          </span>
        </button>;
      })}
    </div>
  </section>;
}

export function JobsDashboard({ api, jobs, localActivities, focusedJobId, connection, loaded, resultBlocked,
  operationLabel, onBack, onReconnect, onRefresh, onLoadOlder, onViewResult, onStop, onDetails }: {
  api: MeshHarborApi;
  jobs: Job[];
  localActivities: StudioActivity[];
  focusedJobId: string | null;
  connection: 'disconnected' | 'connecting' | 'connected';
  loaded: boolean;
  resultBlocked: boolean;
  operationLabel: (operation: string) => string;
  onBack: () => void;
  onReconnect: () => void;
  onRefresh: () => Promise<void>;
  onLoadOlder: (before: string) => Promise<Job[]>;
  onViewResult: (id: string) => void;
  onStop: (job: Job, trigger: HTMLButtonElement) => void;
  onDetails: (job: Job, trigger: HTMLButtonElement) => void;
}) {
  const [historyCache, setHistoryCache] = useState<Job[]>([]);
  const [oldestReached, setOldestReached] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [queue, setQueue] = useState<string[]>([]);
  const [reordering, setReordering] = useState(false);
  const queueRevision = useRef(0);
  const queueEditing = useRef(false);
  const heading = useRef<HTMLHeadingElement>(null);
  const focusRow = useRef<HTMLDivElement>(null);
  const queueById = new Map(jobs.filter(job => isGenerationOperation(job.operation) && job.state === 'queued').map(job => [job.id, job]));
  const hasQueuedGeneration = queueById.size > 0;
  const queued = queue.flatMap(id => { const job = queueById.get(id); return job ? [job] : []; });
  const queuedIds = new Set(queued.map(job => job.id));
  const active = jobs.filter(job => isActiveJob(job) && !queuedIds.has(job.id));
  const history = useMemo(() => mergeJobRecords(jobs.filter(job => !isActiveJob(job)), historyCache), [jobs, historyCache]);
  const focused = jobs.some(job => job.id === focusedJobId) || history.some(job => job.id === focusedJobId);
  const activeCount = active.length + localActivities.length;
  const now = useJobClock(activeCount > 0 || queued.length > 0);
  useEffect(() => { heading.current?.focus(); }, []);
  useEffect(() => {
    setHistoryCache(current => mergeJobRecords(jobs.filter(job => !isActiveJob(job)), current));
  }, [jobs]);
  useEffect(() => {
    if (focused) focusRow.current?.scrollIntoView({ block: 'nearest' });
  }, [focused, focusedJobId]);

  useEffect(() => {
    if (connection !== 'connected' || !hasQueuedGeneration) {
      setQueue([]);
      return;
    }
    const controller = new AbortController();
    let timer = 0;
    async function poll() {
      const revision = queueRevision.current;
      if (!queueEditing.current) {
        try {
          const order = await api.generationQueue(controller.signal);
          if (!controller.signal.aborted && revision === queueRevision.current) setQueue(order);
        } catch {
          if (!controller.signal.aborted && revision === queueRevision.current) setQueue([]);
        }
      }
      if (!controller.signal.aborted) timer = window.setTimeout(poll, 1000);
    }
    void poll();
    return () => { controller.abort(); window.clearTimeout(timer); };
  }, [api, connection, hasQueuedGeneration]);

  async function moveQueued(job: Job, neighbor: Job) {
    if (queueEditing.current) return;
    queueEditing.current = true;
    queueRevision.current += 1;
    setReordering(true);
    try {
      setQueue(await api.swapGenerationQueue(job.id, neighbor.id));
      notify('success', 'Generation queue updated.');
    } catch (cause) {
      notify('error', 'Could not update the generation queue.', cause instanceof Error ? cause.message : undefined);
    } finally {
      queueEditing.current = false;
      setReordering(false);
    }
  }

  async function loadOlder() {
    const before = history.at(-1)?.id;
    if (!before || loadingOlder) return;
    setLoadingOlder(true);
    try {
      const next = await onLoadOlder(before);
      setHistoryCache(current => mergeJobRecords(current, next));
      setOldestReached(next.length < JOB_PAGE_SIZE);
    } catch (cause) {
      notify('error', cause instanceof Error ? cause.message : 'Could not load older jobs.');
    } finally {
      setLoadingOlder(false);
    }
  }

  function renderJob(job: Job, position?: number) {
    const { label, duration, queuedFor } = jobTiming(job, now);
    const lastStage = job.state === 'failed' ? lastGenerationStage(job) : undefined;
    const selected = job.id === focusedJobId;
    return <JobListItem key={job.id} job={job} title={operationLabel(job.operation)} showUpdated selected={selected} onDetails={onDetails}
      detail={<div ref={selected ? focusRow : undefined} className="grid min-w-0 gap-1">
        {job.input_asset_name && <ItemDescription className="line-clamp-none wrap-anywhere">{job.input_asset_name}</ItemDescription>}
        <ItemDescription className="line-clamp-none wrap-anywhere">
          {position !== undefined ? `#${position + 1} in queue` : jobStage(job)}{lastStage && ` · Last stage: ${lastStage}`}
        </ItemDescription>
        <p className="flex flex-wrap gap-x-2 tabular-nums text-muted-foreground">
          <span>{label}: {duration}</span>
          {queuedFor && <span>Queued for: {queuedFor}</span>}
        </p>
      </div>}>
      {position !== undefined && <>
        <Button variant="ghost" size="icon-sm" aria-label="Move earlier" disabled={position === 0 || reordering || connection !== 'connected'}
          onClick={() => void moveQueued(job, queued[position - 1])}><ArrowUpIcon aria-hidden="true" /></Button>
        <Button variant="ghost" size="icon-sm" aria-label="Move later" disabled={position === queued.length - 1 || reordering || connection !== 'connected'}
          onClick={() => void moveQueued(job, queued[position + 1])}><ArrowDownIcon aria-hidden="true" /></Button>
      </>}
      {isJobCancellable(job) && <Button variant="outline" size="sm" disabled={connection !== 'connected'} onClick={event => onStop(job, event.currentTarget)}>{position !== undefined ? 'Remove from queue' : 'Stop job'}</Button>}
      {job.state === 'running' && job.operation.startsWith('runtime.') &&
        <HelpTip label="About stopping setup jobs">Setup jobs can only be stopped while queued.</HelpTip>}
      {job.state === 'succeeded' && job.result?.asset_id && <Button variant="ghost" size="sm" onClick={() => onViewResult(job.id)}
        disabled={resultBlocked || connection !== 'connected'}>View result <ArrowUpRightIcon aria-hidden="true" /></Button>}
    </JobListItem>;
  }

  return <div className="min-h-dvh bg-background">
    <header className="flex min-h-12 items-center gap-3 border-b px-4 py-2 sm:px-6">
      <Button variant="ghost" size="sm" onClick={onBack}><ArrowLeftIcon aria-hidden="true" /> Studio</Button>
      <span className="font-heading text-sm font-medium">meshharbor</span>
      <div className="ml-auto"><NotificationHistory /></div>
    </header>
    <main className="mx-auto grid w-full max-w-4xl gap-5 px-4 py-6 sm:px-6 sm:py-9">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="grid gap-1">
          <h1 id="jobs-dashboard-title" ref={heading} tabIndex={-1} className="font-heading text-2xl font-medium outline-none sm:text-3xl">Jobs dashboard</h1>
        </div>
        <Button variant="outline" size="sm" disabled={refreshing || connection === 'connecting'} onClick={() => {
          if (connection === 'disconnected') onReconnect();
          else { setRefreshing(true); void onRefresh().finally(() => setRefreshing(false)); }
        }}>
          {refreshing || connection === 'connecting' ? <Spinner aria-hidden="true" /> : <ArrowsClockwiseIcon aria-hidden="true" />}
          {connection === 'disconnected' ? 'Reconnect' : 'Refresh'}
        </Button>
      </div>
      <Card size="sm" role="region" aria-labelledby="jobs-active">
        <CardHeader>
          <CardTitle id="jobs-active" className="flex items-center gap-2" role="heading" aria-level={2}>
            In progress <Badge variant="secondary">{activeCount}</Badge>
            {active.length > 0 && <HelpTip label="About interrupted jobs">
              Jobs interrupted by an engine exit may appear here until recovered. Viewing this page never resumes them.
            </HelpTip>}
          </CardTitle>
        </CardHeader>
        <CardContent className="grid gap-3">
          {activeCount > 0 ? <ItemGroup>
            {localActivities.map(activity => <Item key={activity.title} role="listitem" variant="outline" size="xs">
              <ItemContent>
                <ItemTitle>{activity.title}</ItemTitle>
                <ItemDescription className="line-clamp-none wrap-anywhere">{activity.stage}</ItemDescription>
                <p className="tabular-nums text-muted-foreground">Elapsed: {formatDuration(now - activity.startedAt)}</p>
              </ItemContent>
              <ItemActions><Badge variant="outline"><Spinner aria-hidden="true" /> Running</Badge></ItemActions>
            </Item>)}
            {active.map(job => renderJob(job))}
          </ItemGroup> : <Empty>
            <EmptyHeader>
              <EmptyMedia variant="icon"><ListChecksIcon aria-hidden="true" /></EmptyMedia>
              <EmptyTitle>{loaded ? 'Nothing in progress' : connection === 'disconnected' ? 'Engine unavailable' : 'Checking the local engine…'}</EmptyTitle>
            </EmptyHeader>
          </Empty>}
        </CardContent>
      </Card>
      {queued.length > 0 && <Card size="sm" role="region" aria-labelledby="jobs-queue">
        <CardHeader><CardTitle id="jobs-queue" role="heading" aria-level={2} className="flex items-center gap-2">
          Generation queue <Badge variant="secondary">{queued.length}</Badge>
        </CardTitle></CardHeader>
        <CardContent><ItemGroup>{queued.map((job, position) => renderJob(job, position))}</ItemGroup></CardContent>
      </Card>}
      <Card size="sm" role="region" aria-labelledby="jobs-history">
        <CardHeader><CardTitle id="jobs-history" role="heading" aria-level={2}>History</CardTitle></CardHeader>
        <CardContent className="grid gap-3">
          {history.length > 0 ? <ItemGroup>{history.map(job => renderJob(job))}</ItemGroup> : <Empty>
            <EmptyHeader><EmptyDescription>{loaded ? 'No finished jobs yet.' : 'Job history is not available yet.'}</EmptyDescription></EmptyHeader>
          </Empty>}
          {!oldestReached && history.length >= JOB_PAGE_SIZE && <Button variant="outline" size="sm" className="justify-self-start"
            disabled={loadingOlder || connection !== 'connected'} onClick={() => void loadOlder()}>{loadingOlder ? 'Loading…' : 'Load older jobs'}</Button>}
        </CardContent>
      </Card>
    </main>
  </div>;
}
