import * as CollapsiblePrimitive from '@radix-ui/react-collapsible';
import { forwardRef } from 'react';
import './shadcn.css';
import { cn } from './utils';

export const Collapsible = forwardRef<
  React.ElementRef<typeof CollapsiblePrimitive.Root>,
  React.ComponentPropsWithoutRef<typeof CollapsiblePrimitive.Root>
>(function Collapsible({ className, ...props }, ref) {
  return <CollapsiblePrimitive.Root ref={ref} className={cn('mh-shadcn-theme', 'shadcn-collapsible', className)} {...props} data-slot="collapsible" />;
});

export const CollapsibleTrigger = forwardRef<
  React.ElementRef<typeof CollapsiblePrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof CollapsiblePrimitive.Trigger>
>(function CollapsibleTrigger({ className, ...props }, ref) {
  return <CollapsiblePrimitive.Trigger ref={ref} className={cn('shadcn-collapsible-trigger', className)} {...props} data-slot="collapsible-trigger" />;
});

export const CollapsibleContent = forwardRef<
  React.ElementRef<typeof CollapsiblePrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof CollapsiblePrimitive.Content>
>(function CollapsibleContent({ className, ...props }, ref) {
  return <CollapsiblePrimitive.Content ref={ref} className={cn('shadcn-collapsible-content', className)} {...props} data-slot="collapsible-content" />;
});

Collapsible.displayName = 'Collapsible';
CollapsibleTrigger.displayName = 'CollapsibleTrigger';
CollapsibleContent.displayName = 'CollapsibleContent';
