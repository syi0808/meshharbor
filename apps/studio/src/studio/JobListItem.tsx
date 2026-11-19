import type { ReactNode } from 'react';
import { Badge } from '@/components/ui/badge';
import { Item, ItemActions, ItemContent, ItemDescription, ItemTitle } from '@/components/ui/item';
import type { Job } from './types';

export function JobListItem({ job, title, selected = false, showUpdated, progress, children }: {
  job: Job;
  title: string;
  selected?: boolean;
  showUpdated?: boolean;
  progress?: number;
  children?: ReactNode;
}) {
  const state = job.state === 'succeeded' ? 'Complete' : job.state[0].toUpperCase() + job.state.slice(1);
  const updatedAt = new Date(Number(job.updated_at));
  return <Item role="listitem" variant="outline" size="xs" className="min-w-0 gap-2 data-[selected=true]:bg-muted" data-selected={selected}>
    <ItemContent className="min-w-24">
      <ItemTitle className="max-w-full wrap-anywhere">{title}</ItemTitle>
      {showUpdated && <ItemDescription>Updated {Number.isNaN(updatedAt.getTime()) ? 'time unavailable' : updatedAt.toLocaleString()}</ItemDescription>}
      {job.error?.message && <ItemDescription className="line-clamp-none wrap-anywhere text-destructive">{job.error.message}</ItemDescription>}
    </ItemContent>
    <ItemActions className="ml-auto flex-wrap justify-end gap-1">
      <Badge variant={job.state === 'failed' ? 'destructive' : job.state === 'succeeded' ? 'secondary' : 'outline'}>{state}</Badge>
      {progress !== undefined && <span className="tabular-nums text-muted-foreground">{progress}%</span>}
      {children}
    </ItemActions>
  </Item>;
}
