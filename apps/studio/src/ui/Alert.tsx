import { forwardRef, type HTMLAttributes } from 'react';
import './shadcn.css';
import { cn } from './utils';

export type AlertVariant = 'default' | 'destructive';
export interface AlertProps extends HTMLAttributes<HTMLDivElement> {
  variant?: AlertVariant;
}

export const Alert = forwardRef<HTMLDivElement, AlertProps>(function Alert(
  { className, variant = 'default', role = 'alert', ...props },
  ref,
) {
  return <div ref={ref} role={role} className={cn('mh-shadcn-theme', 'shadcn-alert', `shadcn-alert--${variant}`, className)} {...props} data-slot="alert" />;
});

export const AlertTitle = forwardRef<HTMLHeadingElement, HTMLAttributes<HTMLHeadingElement>>(
  function AlertTitle({ className, ...props }, ref) {
    return <h5 ref={ref} className={cn('shadcn-alert-title', className)} {...props} data-slot="alert-title" />;
  },
);

export const AlertDescription = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function AlertDescription({ className, ...props }, ref) {
    return <div ref={ref} className={cn('shadcn-alert-description', className)} {...props} data-slot="alert-description" />;
  },
);

export const AlertAction = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function AlertAction({ className, ...props }, ref) {
    return <div ref={ref} className={cn('shadcn-alert-action', className)} {...props} data-slot="alert-action" />;
  },
);

Alert.displayName = 'Alert';
AlertTitle.displayName = 'AlertTitle';
AlertDescription.displayName = 'AlertDescription';
AlertAction.displayName = 'AlertAction';
