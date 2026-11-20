import { BellIcon, XIcon } from '@phosphor-icons/react';
import { useId } from 'react';
import { Popover } from 'radix-ui';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { useNotificationHistory } from './notify';

const toneLabels = { success: 'Success', info: 'Info', warning: 'Warning', error: 'Error' };
const formatTime = new Intl.DateTimeFormat(undefined, { dateStyle: 'short', timeStyle: 'short' });

export function NotificationHistory() {
  const headingId = useId();
  const history = useNotificationHistory();

  return <Popover.Root>
    <Popover.Trigger asChild>
      <Button type="button" variant="ghost" size="icon-sm" aria-label="Notifications" title="Notifications">
        <BellIcon aria-hidden="true" />
      </Button>
    </Popover.Trigger>
    <Popover.Portal>
      <Popover.Content align="end" sideOffset={8} collisionPadding={12} aria-labelledby={headingId}
        className="z-50 flex max-h-[min(24rem,var(--radix-popover-content-available-height))] w-80 max-w-(--radix-popover-content-available-width) flex-col overflow-hidden rounded-xl border bg-popover text-xs/relaxed text-popover-foreground shadow-lg outline-none">
        <div className="flex shrink-0 items-center justify-between gap-2 border-b px-3 py-2">
          <h2 id={headingId} className="font-heading text-sm font-medium">Notification history</h2>
          <Popover.Close asChild>
            <Button type="button" variant="ghost" size="icon-xs" aria-label="Close notification history"><XIcon aria-hidden="true" /></Button>
          </Popover.Close>
        </div>
        {history.length === 0 ? <p className="p-3 text-muted-foreground">No notifications yet.</p>
          : <ol tabIndex={0} aria-label="Recent notifications"
            className="min-h-0 divide-y overflow-y-auto overscroll-contain outline-none focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:ring-inset">
            {history.map(entry => (
              <li key={entry.id} className="grid min-w-0 gap-1 p-3 wrap-anywhere">
                <div className="flex flex-wrap items-center justify-between gap-1">
                  <Badge variant={entry.tone === 'error' ? 'destructive' : 'outline'}>{toneLabels[entry.tone]}</Badge>
                  <time dateTime={new Date(entry.timestamp).toISOString()} className="text-muted-foreground">{formatTime.format(entry.timestamp)}</time>
                </div>
                <p className="font-medium">{entry.message}</p>
                {entry.description && <p className="text-muted-foreground">{entry.description}</p>}
              </li>
            ))}
          </ol>}
      </Popover.Content>
    </Popover.Portal>
  </Popover.Root>;
}
