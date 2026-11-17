import { forwardRef, type ButtonHTMLAttributes } from 'react';
import './shadcn.css';
import { cn } from './utils';

export type ButtonVariant = 'default' | 'secondary' | 'ghost' | 'outline' | 'destructive';
export type ButtonSize = 'default' | 'sm' | 'lg' | 'icon' | 'xs' | 'icon-xs' | 'icon-sm' | 'icon-lg';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
}

/** A shadcn-style button primitive, adapted to Studio's CSS-token theme. */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, variant = 'default', size = 'default', type = 'button', ...props },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type}
      className={cn('mh-shadcn-theme', 'shadcn-button', `shadcn-button--${variant}`, `shadcn-button-size--${size}`, className)}
      {...props}
      data-slot="button"
    />
  );
});

Button.displayName = 'Button';
