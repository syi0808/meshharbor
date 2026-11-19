import { toast } from 'sonner';

const durations = {
  success: 4500,
  info: 5500,
  warning: 9000,
  error: 9000,
};

export function notify(tone: keyof typeof durations, message: string) {
  toast[tone](message, { id: `${tone}:${message}`, duration: durations[tone] });
}
