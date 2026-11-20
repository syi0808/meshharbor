import { useRef } from 'react';
import { Badge } from '@/components/ui/badge';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { jobStage, jobStateLabel, lastGenerationStage, type JobEvent } from './job-activity';
import type { Job } from './types';

function JobTime({ value }: { value: string }) {
  const date = new Date(Number(value));
  return /^\d+$/.test(value) && !Number.isNaN(date.getTime())
    ? <time dateTime={date.toISOString()}>{date.toLocaleString()}</time> : <span>Unavailable</span>;
}

export function JobDetails({ job, title, events, onClose, returnFocus }: {
  job: Job;
  title: string;
  events?: JobEvent[];
  onClose: () => void;
  returnFocus: () => void;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  const timeline = events?.length ? events : [{ state: job.state, stage: jobStage(job), at: job.updated_at }];
  const lastStage = lastGenerationStage(job);
  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}>
    <DialogContent className="max-h-[calc(100dvh-2rem)] grid-rows-[auto_minmax(0,1fr)] sm:max-w-md"
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
          {lastStage && job.state !== 'running' && <><dt>Last stage</dt><dd>{lastStage}</dd></>}
          <dt>Job ID</dt><dd className="wrap-anywhere"><code>{job.id}</code></dd>
        </dl>
        {job.error?.message && <p className="rounded-md border border-destructive/30 p-2 wrap-anywhere whitespace-pre-wrap text-destructive">{job.error.message}</p>}
        <div>
          <h3 className="font-medium">Activity log</h3>
          <p className="text-muted-foreground">Status changes observed in this Studio session.</p>
          <ol className="mt-2 divide-y rounded-md border">
            {timeline.map((entry, index) => <li key={`${entry.at}-${index}`} className="grid gap-0.5 px-2 py-1.5">
              <span>{jobStateLabel(entry.state)} · {entry.stage}</span>
              <span className="text-muted-foreground"><JobTime value={entry.at} /></span>
            </li>)}
          </ol>
        </div>
      </div>
    </DialogContent>
  </Dialog>;
}
