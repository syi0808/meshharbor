import type { ReactNode } from 'react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Item, ItemActions, ItemContent, ItemDescription, ItemTitle } from '@/components/ui/item';
import { notify } from './notify';
import type { Job } from './types';

export function JobListItem({ job, title, selected = false, showUpdated, detail, children }: {
  job: Job;
  title: string;
  selected?: boolean;
  showUpdated?: boolean;
  detail?: ReactNode;
  children?: ReactNode;
}) {
  const state = job.state === 'succeeded' ? 'Complete' : job.state === 'cancelling' ? 'Stopping' : job.state[0].toUpperCase() + job.state.slice(1);
  const updatedAt = new Date(Number(job.updated_at));
  return <Item role="listitem" variant="outline" size="xs" className="min-w-0 gap-2 data-[selected=true]:bg-muted" data-selected={selected}>
    <ItemContent className="min-w-24">
      <ItemTitle className="max-w-full wrap-anywhere">{title}</ItemTitle>
      {detail}
      {showUpdated && <ItemDescription>Updated {Number.isNaN(updatedAt.getTime()) ? 'time unavailable' : updatedAt.toLocaleString()}</ItemDescription>}
    </ItemContent>
    <ItemActions className="ml-auto flex-wrap justify-end gap-1">
      <Badge variant={job.state === 'failed' ? 'destructive' : job.state === 'succeeded' ? 'secondary' : 'outline'}>{state}</Badge>
      {job.state === 'failed' && <Button variant="ghost" size="sm" aria-label={`Show error for ${title}`}
        onClick={() => notify('error', 'Job failed.', job.error?.message)}>Details</Button>}
      {children}
    </ItemActions>
  </Item>;
}
