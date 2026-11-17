import { forwardRef, type LabelHTMLAttributes } from 'react';
import './shadcn.css';
import { cn } from './utils';

export type LabelProps = LabelHTMLAttributes<HTMLLabelElement>;

export const Label = forwardRef<HTMLLabelElement, LabelProps>(function Label({ className, ...props }, ref) {
  return <label ref={ref} className={cn('mh-shadcn-theme', 'shadcn-label', className)} {...props} data-slot="label" />;
});

Label.displayName = 'Label';
