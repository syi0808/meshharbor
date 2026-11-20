import { useEffect, useRef, useState } from 'react';
import { Badge } from '@/components/ui/badge';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { isActiveJob, type MeshHarborApi } from './api';
import { jobStage, jobStateLabel, jobStepLabel } from './job-activity';
import type { Job, JobCommand, JobStep } from './types';

function JobTime({ value }: { value: string }) {
  const date = new Date(Number(value));
  return /^\d+$/.test(value) && !Number.isNaN(date.getTime())
    ? <time dateTime={date.toISOString()}>{date.toLocaleString()}</time> : <span>Unavailable</span>;
}

function JobLog({ children }: { children: string }) {
  return <pre tabIndex={0} className="max-h-48 overflow-auto rounded-md bg-muted p-2 font-mono text-xs whitespace-pre-wrap wrap-anywhere">{children.replace(/\r\n?/g, '\n')}</pre>;
}

function CommandDetails({ command, active }: { command: JobCommand; active: boolean }) {
  const display = command.argv.map(arg => /^[\w./:=@+-]+$/.test(arg) ? arg : `'${arg.replaceAll("'", "'\\''")}'`).join(' ');
  return <div className="grid min-w-0 gap-1 rounded-md border p-2">
    <p className="font-medium">Command</p>
    <JobLog>{display}</JobLog>
    {command.cwd && <p className="text-xs wrap-anywhere">Working directory: <code>{command.cwd}</code></p>}
    <p className="text-xs text-muted-foreground"><JobTime value={command.started_at} /> · {command.outcome ?? (active ? 'Running' : 'No exit status recorded')}</p>
    {command.stdout && <><p className="font-medium">Stdout</p><JobLog>{command.stdout}</JobLog></>}
    {command.stderr && <><p className="font-medium">Stderr</p><JobLog>{command.stderr}</JobLog></>}
    {!command.stdout && !command.stderr && <p className="text-muted-foreground">No output recorded.</p>}
    {command.truncated && <p className="text-muted-foreground">Earlier output was truncated.</p>}
  </div>;
}

export function JobDetails({ api, job, title, onClose, returnFocus }: {
  api: MeshHarborApi;
  job: Job;
  title: string;
  onClose: () => void;
  returnFocus: () => void;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  const [steps, setSteps] = useState<JobStep[] | null>(null);
  const [logError, setLogError] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const active = isActiveJob(job);
  const jobId = job.id;
  useEffect(() => {
    const request = new AbortController();
    let timer = 0;
    async function poll() {
      try {
        const saved = await api.jobSteps(jobId, request.signal);
        if (!request.signal.aborted) {
          setSteps(saved);
          setLogError(false);
        }
      } catch {
        if (!request.signal.aborted) setLogError(true);
      } finally {
        if (active && !request.signal.aborted) timer = window.setTimeout(poll, 1500);
      }
    }
    void poll();
    return () => { request.abort(); window.clearTimeout(timer); };
  }, [active, api, jobId, refresh]);
  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}>
    <DialogContent className="max-h-[calc(100dvh-2rem)] grid-rows-[auto_minmax(0,1fr)] sm:max-w-2xl"
      onOpenAutoFocus={event => { event.preventDefault(); heading.current?.focus(); }}
      onCloseAutoFocus={event => { event.preventDefault(); returnFocus(); }}>
      <DialogHeader className="min-w-0 pr-6">
        <DialogTitle ref={heading} tabIndex={-1} className="outline-none">Job details</DialogTitle>
        <DialogDescription className="wrap-anywhere">{title}{job.input_asset_name && ` · ${job.input_asset_name}`}</DialogDescription>
      </DialogHeader>
      <div tabIndex={0} className="grid min-h-0 gap-3 overflow-auto overscroll-contain">
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant={job.state === 'failed' ? 'destructive' : 'outline'}>{jobStateLabel(job.state)}</Badge>
          <span>{jobStage(job)}</span>
        </div>
        <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-muted-foreground">
          <dt>Submitted</dt><dd><JobTime value={job.created_at} /></dd>
          {job.started_at && <><dt>Started</dt><dd><JobTime value={job.started_at} /></dd></>}
          {job.stage && job.state !== 'running' && <><dt>Last stage</dt><dd>{jobStepLabel(job.stage)}</dd></>}
          <dt>Job ID</dt><dd className="wrap-anywhere"><code>{job.id}</code></dd>
        </dl>
        {job.error?.message && <p className="rounded-md border border-destructive/30 p-2 wrap-anywhere whitespace-pre-wrap text-destructive">{job.error.message}</p>}
        {job.parameters && Object.keys(job.parameters).length > 0 && <details className="rounded-md border p-2">
          <summary className="cursor-pointer font-medium">Parameters</summary>
          <JobLog>{JSON.stringify(job.parameters, null, 2)}</JobLog>
        </details>}
        <div className="min-w-0">
          <h3 className="font-medium">Execution log</h3>
          <p className="text-muted-foreground">Saved in the local engine. Expand a step to see its details.</p>
          {logError && <p className="text-muted-foreground">Could not load execution logs.{' '}
            <button type="button" className="underline" onClick={() => setRefresh(value => value + 1)}>Retry</button></p>}
          {!logError && steps === null && <p className="text-muted-foreground">Loading execution logs…</p>}
          {!logError && steps?.length === 0 && <p className="text-muted-foreground">No execution steps were recorded for this job.</p>}
          {!!steps?.length && <ol className="mt-2 divide-y rounded-md border">
            {steps.map(step => <li key={step.id} className="min-w-0 px-2 py-1.5">
              <details>
                <summary className="cursor-pointer font-medium">{step.stage ? jobStepLabel(step.stage) : jobStateLabel(step.state)}
                  {' · '}<span className="font-normal text-muted-foreground"><JobTime value={step.at} /></span>
                </summary>
                <div className="grid min-w-0 gap-2 pt-2">
                  {step.message && <p className="wrap-anywhere">{step.message}</p>}
                  {step.commands.map(command => <CommandDetails key={command.id} command={command} active={active} />)}
                  {!step.message && !step.commands.length && <p className="text-muted-foreground">No command output for this step.</p>}
                </div>
              </details>
            </li>)}
          </ol>}
        </div>
      </div>
    </DialogContent>
  </Dialog>;
}
