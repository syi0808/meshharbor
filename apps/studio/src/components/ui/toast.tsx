import { CheckCircleIcon, InfoIcon, WarningCircleIcon, WarningIcon, XIcon } from '@phosphor-icons/react';
import { Toast } from 'radix-ui';
import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from 'react';
import { Button } from '@/components/ui/button';

const appearances = {
  success: { Icon: CheckCircleIcon, color: 'text-primary', duration: 4500 },
  info: { Icon: InfoIcon, color: 'text-muted-foreground', duration: 5500 },
  warning: { Icon: WarningIcon, color: 'text-amber-700 dark:text-amber-300', duration: 9000 },
  error: { Icon: WarningCircleIcon, color: 'text-destructive', duration: 9000 },
};

type Tone = keyof typeof appearances;
type Notify = (tone: Tone, message: string) => void;
type Notification = { id: number; tone: Tone; message: string };

const ToastContext = createContext<Notify | null>(null);
const MAX_NOTIFICATIONS = 3;

export function useToast() {
  const notify = useContext(ToastContext);
  if (!notify) throw new Error('useToast requires a ToastProvider.');
  return notify;
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const sequence = useRef(0);
  const [notifications, setNotifications] = useState<Notification[]>([]);
  const notify = useCallback<Notify>((tone, message) => {
    const id = ++sequence.current;
    setNotifications(current => [
      ...current.filter(item => item.tone !== tone || item.message !== message).slice(1 - MAX_NOTIFICATIONS),
      { id, tone, message },
    ]);
  }, []);

  return <ToastContext.Provider value={notify}>
    <Toast.Provider>
      {children}
      {notifications.map(({ id, tone, message }) => {
        const { Icon, color, duration } = appearances[tone];
        return <Toast.Root key={id} open duration={duration} data-tone={tone}
          onOpenChange={open => { if (!open) setNotifications(current => current.filter(item => item.id !== id)); }}
          className="pointer-events-none grid grid-cols-[auto_minmax(0,1fr)_auto] items-start gap-2 rounded-lg border bg-popover px-3 py-2.5 text-xs/relaxed text-popover-foreground shadow-lg">
          <Icon aria-hidden="true" className={`mt-0.5 size-4 ${color}`} />
          <Toast.Description className="wrap-anywhere">{message}</Toast.Description>
          <Toast.Close asChild><Button type="button" variant="ghost" size="icon-sm" className="pointer-events-auto" aria-label="Dismiss notification">
            <XIcon aria-hidden="true" />
          </Button></Toast.Close>
        </Toast.Root>;
      })}
      <Toast.Viewport
        label="Notifications ({hotkey})"
        className="pointer-events-none fixed right-3 bottom-16 z-60 grid max-h-[60dvh] w-[calc(100%_-_1.5rem)] max-w-[22rem] gap-2 overflow-y-auto p-1 outline-none min-[761px]:bottom-3" />
    </Toast.Provider>
  </ToastContext.Provider>;
}
