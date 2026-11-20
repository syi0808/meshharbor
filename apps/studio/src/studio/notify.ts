import { useSyncExternalStore } from 'react';
import { toast } from 'sonner';

const durations = {
  success: 4500,
  info: 5500,
  warning: 9000,
  error: 9000,
};

type NotificationTone = keyof typeof durations;
type Notification = Readonly<{
  id: number;
  tone: NotificationTone;
  message: string;
  description?: string;
  timestamp: number;
}>;

const historyLimit = 50;
const listeners = new Set<() => void>();
let history: readonly Notification[] = [];
let nextId = 0;

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function getHistory() {
  return history;
}

export function useNotificationHistory() {
  return useSyncExternalStore(subscribe, getHistory);
}

export function notify(tone: NotificationTone, message: string, description?: string) {
  toast[tone](message, { id: `${tone}:${message}:${description ?? ''}`, duration: durations[tone], description });
  const entry = { id: ++nextId, tone, message, description, timestamp: Date.now() };
  history = [entry, ...history.slice(0, historyLimit - 1)];
  listeners.forEach(listener => listener());
}
