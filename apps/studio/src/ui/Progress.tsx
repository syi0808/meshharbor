import { forwardRef, type HTMLAttributes } from 'react';
import './shadcn.css';
import { cn } from './utils';

export interface ProgressProps extends Omit<HTMLAttributes<HTMLDivElement>, 'value'> {
  value?: number | null;
  max?: number;
}

export const Progress = forwardRef<HTMLDivElement, ProgressProps>(function Progress(
  { className, value = 0, max = 100, ...props },
  ref,
) {
  const maximum = Number.isFinite(max) && max > 0 ? max : 100;
  const determinate = value !== null && Number.isFinite(value);
  const safeValue = determinate ? Math.min(maximum, Math.max(0, value)) : null;
  const percent = safeValue === null ? undefined : (safeValue / maximum) * 100;

  return (
    <div
      ref={ref}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={maximum}
      aria-valuenow={safeValue ?? undefined}
      data-state={safeValue === null ? 'indeterminate' : safeValue === maximum ? 'complete' : 'loading'}
      className={cn('mh-shadcn-theme', 'shadcn-progress', className)}
      {...props}
      data-slot="progress"
    >
      <span data-slot="progress-indicator" className="shadcn-progress-indicator" style={percent === undefined ? undefined : { transform: `translateX(-${100 - percent}%)` }} />
    </div>
  );
});

Progress.displayName = 'Progress';
