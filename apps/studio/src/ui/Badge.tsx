import { forwardRef, type HTMLAttributes } from 'react';
import './shadcn.css';
import { cn } from './utils';

export type BadgeVariant = 'default' | 'secondary' | 'outline' | 'destructive' | 'success' | 'warning';
export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  variant?: BadgeVariant;
}

export const Badge = forwardRef<HTMLSpanElement, BadgeProps>(function Badge(
  { className, variant = 'default', ...props },
  ref,
) {
  return <span ref={ref} className={cn('mh-shadcn-theme', 'shadcn-badge', `shadcn-badge--${variant}`, className)} {...props} data-slot="badge" />;
});

Badge.displayName = 'Badge';
