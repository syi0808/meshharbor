import * as DialogPrimitive from '@radix-ui/react-dialog';
import type { ReactNode } from 'react';
import './shadcn.css';
import { cn } from './utils';

export interface SheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  portal: HTMLElement | null;
  children: ReactNode;
  onCloseAutoFocus?: (event: Event) => void;
  side?: 'left' | 'right' | 'top' | 'bottom';
  className?: string;
  id?: string;
}

/**
 * Studio's shadcn-style Sheet. Radix Dialog preserves focus, Escape and
 * outside-dismissal behavior; left-side legacy hooks stay in place.
 */
export function Sheet({
  open,
  onOpenChange,
  title,
  portal,
  children,
  onCloseAutoFocus,
  side = 'left',
  className,
  id = 'workbench-sidebar-drawer',
}: SheetProps) {
  return (
    <DialogPrimitive.Root open={open} onOpenChange={onOpenChange}>
      <DialogPrimitive.Portal container={portal}>
        <DialogPrimitive.Overlay className="mh-shadcn-theme workbench-mobile-scrim shadcn-sheet-overlay" />
        <DialogPrimitive.Content
          className={cn('mh-shadcn-theme', 'workbench-dock', 'workbench-sheet', 'shadcn-sheet', `shadcn-sheet--${side}`, className)}
          data-side={side}
          id={id}
          aria-describedby={undefined}
          onCloseAutoFocus={onCloseAutoFocus}
          aria-modal={true}
          data-slot="sheet-content"
        >
          <DialogPrimitive.Title className="sr-only">{title}</DialogPrimitive.Title>
          {children}
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

export const SheetClose = DialogPrimitive.Close;
