import { ArrowLeftIcon, ArrowUpRightIcon, ArrowsClockwiseIcon, CubeIcon, ListChecksIcon } from '@phosphor-icons/react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty';
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import { isActiveJob, isJobCancellable } from './api';
import { HelpTip } from './HelpTip';
import { JOB_PAGE_SIZE, formatDuration, jobStage, jobTiming, lastGenerationStage, mergeJobRecords } from './job-activity';
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

export function PendingGenerationCards({ jobs, sources, onOpen }: {
  jobs: Job[];
  sources: ReadonlyMap<string, string>;
  onOpen: (id: string) => void;
}) {
  const now = useJobClock(jobs.length > 0);
  if (!jobs.length) return null;
  return <section aria-label="Models in progress" className="grid min-w-0 gap-2">
    <h3 className="flex items-center gap-2 text-xs font-medium">
      In progress <Badge variant="secondary">{jobs.length}</Badge>
    </h3>
    <div className="grid min-w-0 grid-cols-2 gap-2 @min-[380px]:grid-cols-3 max-[330px]:grid-cols-1">
      {jobs.map(job => {
        const name = job.input_asset_name || (job.input_asset_id && sources.get(job.input_asset_id)) || 'New model';
        const { label, duration } = jobTiming(job, now);
        return <Card key={job.id} size="sm" className="min-w-0 gap-0 pb-0 pt-0">
          <div className="relative grid aspect-square w-full place-items-center overflow-hidden border-b bg-muted/60">
            <CubeIcon className="size-10 text-primary/60" weight="duotone" aria-hidden="true" />
            <Badge className="absolute inset-x-1.5 bottom-1.5 mx-auto max-w-full" variant="secondary">
              <Spinner aria-hidden="true" /> {job.state === 'queued' ? 'Queued' : job.state === 'cancelling' ? 'Stopping' : 'Generating'}
            </Badge>
          </div>
          <CardContent className="grid min-w-0 gap-1 p-2">
            <p className="truncate font-medium" title={name}>{name}</p>
            <p className="truncate text-muted-foreground" title={jobStage(job)}>{jobStage(job)}</p>
            <p className="tabular-nums text-muted-foreground">{label}: {duration}</p>
            <Button variant="ghost" size="xs" className="mt-0.5 w-full justify-between"
              aria-label={`View progress for ${name}`} onClick={() => onOpen(job.id)}>
              Details <ArrowUpRightIcon aria-hidden="true" />
            </Button>
          </CardContent>
        </Card>;
      })}
    </div>
  </section>;
}

export function JobsDashboard({ jobs, localActivities, focusedJobId, connection, loaded, resultBlocked,
  operationLabel, onBack, onReconnect, onRefresh, onLoadOlder, onViewResult, onStop }: {
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
}) {
  const [historyCache, setHistoryCache] = useState<Job[]>([]);
  const [oldestReached, setOldestReached] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const heading = useRef<HTMLHeadingElement>(null);
  const focusRow = useRef<HTMLDivElement>(null);
  const active = jobs.filter(isActiveJob);
  const history = useMemo(() => mergeJobRecords(jobs.filter(job => !isActiveJob(job)), historyCache), [jobs, historyCache]);
  const focused = active.some(job => job.id === focusedJobId) || history.some(job => job.id === focusedJobId);
  const activeCount = active.length + localActivities.length;
  const now = useJobClock(activeCount > 0);
  useEffect(() => { heading.current?.focus(); }, []);
  useEffect(() => {
    setHistoryCache(current => mergeJobRecords(jobs.filter(job => !isActiveJob(job)), current));
  }, [jobs]);
  useEffect(() => {
    if (focused) focusRow.current?.scrollIntoView({ block: 'nearest' });
  }, [focused, focusedJobId]);

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

  function renderJob(job: Job) {
    const { label, duration, queuedFor } = jobTiming(job, now);
    const lastStage = job.state === 'failed' ? lastGenerationStage(job) : undefined;
    const selected = job.id === focusedJobId;
    return <JobListItem key={job.id} job={job} title={operationLabel(job.operation)} showUpdated selected={selected}
      detail={<div ref={selected ? focusRow : undefined} className="grid min-w-0 gap-1">
        {job.input_asset_name && <ItemDescription className="line-clamp-none wrap-anywhere">{job.input_asset_name}</ItemDescription>}
        <ItemDescription className="line-clamp-none wrap-anywhere">
          {jobStage(job)}{lastStage && ` · Last stage: ${lastStage}`}
        </ItemDescription>
        <p className="flex flex-wrap gap-x-2 tabular-nums text-muted-foreground">
          <span>{label}: {duration}</span>
          {queuedFor && <span>Queued for: {queuedFor}</span>}
        </p>
      </div>}>
      {isJobCancellable(job) && <Button variant="outline" size="sm" disabled={connection !== 'connected'} onClick={event => onStop(job, event.currentTarget)}>Stop job</Button>}
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
            {active.map(renderJob)}
          </ItemGroup> : <Empty>
            <EmptyHeader>
              <EmptyMedia variant="icon"><ListChecksIcon aria-hidden="true" /></EmptyMedia>
              <EmptyTitle>{loaded ? 'Nothing in progress' : connection === 'disconnected' ? 'Engine unavailable' : 'Checking the local engine…'}</EmptyTitle>
            </EmptyHeader>
          </Empty>}
        </CardContent>
      </Card>
      <Card size="sm" role="region" aria-labelledby="jobs-history">
        <CardHeader><CardTitle id="jobs-history" role="heading" aria-level={2}>History</CardTitle></CardHeader>
        <CardContent className="grid gap-3">
          {history.length > 0 ? <ItemGroup>{history.map(renderJob)}</ItemGroup> : <Empty>
            <EmptyHeader><EmptyDescription>{loaded ? 'No finished jobs yet.' : 'Job history is not available yet.'}</EmptyDescription></EmptyHeader>
          </Empty>}
          {!oldestReached && history.length >= JOB_PAGE_SIZE && <Button variant="outline" size="sm" className="justify-self-start"
            disabled={loadingOlder || connection !== 'connected'} onClick={() => void loadOlder()}>{loadingOlder ? 'Loading…' : 'Load older jobs'}</Button>}
        </CardContent>
      </Card>
    </main>
  </div>;
}
