import * as DialogPrimitive from '@radix-ui/react-dialog';
import { XIcon } from '@phosphor-icons/react';
import { forwardRef, type HTMLAttributes } from 'react';
import './shadcn.css';
import { cn } from './utils';

export const Dialog = DialogPrimitive.Root;
export const DialogTrigger = DialogPrimitive.Trigger;
export const DialogClose = DialogPrimitive.Close;

export const DialogTitle = forwardRef<
  React.ElementRef<typeof DialogPrimitive.Title>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Title>
>(function DialogTitle({ className, ...props }, ref) {
  return <DialogPrimitive.Title ref={ref} className={cn('shadcn-dialog-title', className)} {...props} data-slot="dialog-title" />;
});

export const DialogDescription = forwardRef<
  React.ElementRef<typeof DialogPrimitive.Description>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Description>
>(function DialogDescription({ className, ...props }, ref) {
  return <DialogPrimitive.Description ref={ref} className={cn('shadcn-dialog-description', className)} {...props} data-slot="dialog-description" />;
});

export const DialogOverlay = forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>
>(function DialogOverlay({ className, ...props }, ref) {
  return <DialogPrimitive.Overlay ref={ref} className={cn('mh-shadcn-theme', 'shadcn-overlay', className)} {...props} data-slot="dialog-overlay" />;
});

export interface DialogContentProps extends React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content> {
  /** DOM node to receive the portal. Tokens are also attached to the overlay and content. */
  container?: HTMLElement | null;
  showCloseButton?: boolean;
}

/**
 * Dialog content wraps the Radix portal and overlay so every portal surface
 * receives the isolated Studio semantic theme, including custom containers.
 */
export const DialogContent = forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  DialogContentProps
>(function DialogContent(
  { className, children, container, showCloseButton = true, ...props },
  ref,
) {
  return (
    <DialogPrimitive.Portal container={container}>
      <DialogOverlay />
      <DialogPrimitive.Content
        ref={ref}
        className={cn('mh-shadcn-theme', 'shadcn-dialog-content', className)}
        {...props}
        aria-modal={true}
        data-slot="dialog-content"
      >
        {children}
        {showCloseButton && (
          <DialogPrimitive.Close className="shadcn-dialog-close" aria-label="Close" data-slot="dialog-close">
            <XIcon aria-hidden="true" />
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
});

export const DialogHeader = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function DialogHeader({ className, ...props }, ref) {
    return <div ref={ref} className={cn('shadcn-dialog-header', className)} {...props} data-slot="dialog-header" />;
  },
);

export const DialogFooter = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function DialogFooter({ className, ...props }, ref) {
    return <div ref={ref} className={cn('shadcn-dialog-footer', className)} {...props} data-slot="dialog-footer" />;
  },
);

DialogTitle.displayName = 'DialogTitle';
DialogDescription.displayName = 'DialogDescription';
DialogOverlay.displayName = 'DialogOverlay';
DialogContent.displayName = 'DialogContent';
DialogHeader.displayName = 'DialogHeader';
DialogFooter.displayName = 'DialogFooter';
