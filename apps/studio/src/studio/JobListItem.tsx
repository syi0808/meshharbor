import type { ReactNode } from 'react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Item, ItemActions, ItemContent, ItemDescription, ItemTitle } from '@/components/ui/item';
import { jobStateLabel } from './job-activity';
import type { Job } from './types';

export function JobListItem({ job, title, selected = false, showUpdated, detail, onDetails, children }: {
  job: Job;
  title: string;
  selected?: boolean;
  showUpdated?: boolean;
  detail?: ReactNode;
  onDetails: (job: Job, trigger: HTMLButtonElement) => void;
  children?: ReactNode;
}) {
  const updatedAt = new Date(Number(job.updated_at));
  return <Item role="listitem" variant="outline" size="xs" className="min-w-0 gap-2 data-[selected=true]:bg-muted" data-selected={selected}>
    <ItemContent className="min-w-24">
      <ItemTitle className="max-w-full wrap-anywhere">{title}</ItemTitle>
      {detail}
      {showUpdated && <ItemDescription>Updated {Number.isNaN(updatedAt.getTime()) ? 'time unavailable' : updatedAt.toLocaleString()}</ItemDescription>}
    </ItemContent>
    <ItemActions className="ml-auto flex-wrap justify-end gap-1">
      <Badge variant={job.state === 'failed' ? 'destructive' : job.state === 'succeeded' ? 'secondary' : 'outline'}>{jobStateLabel(job.state)}</Badge>
      <Button variant="ghost" size="sm" aria-label={`View details for ${title}`}
        onClick={event => onDetails(job, event.currentTarget)}>Details</Button>
      {children}
    </ItemActions>
  </Item>;
}
