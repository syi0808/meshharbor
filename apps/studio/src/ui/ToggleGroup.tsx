import * as ToggleGroupPrimitive from '@radix-ui/react-toggle-group';
import { forwardRef } from 'react';
import './shadcn.css';
import { cn } from './utils';

export const ToggleGroup = forwardRef<
  React.ElementRef<typeof ToggleGroupPrimitive.Root>,
  React.ComponentPropsWithoutRef<typeof ToggleGroupPrimitive.Root>
>(function ToggleGroup({ className, ...props }, ref) {
  return <ToggleGroupPrimitive.Root ref={ref} className={cn('mh-shadcn-theme', 'shadcn-toggle-group', className)} {...props} data-slot="toggle-group" />;
});

export const ToggleGroupItem = forwardRef<
  React.ElementRef<typeof ToggleGroupPrimitive.Item>,
  React.ComponentPropsWithoutRef<typeof ToggleGroupPrimitive.Item>
>(function ToggleGroupItem({ className, ...props }, ref) {
  return <ToggleGroupPrimitive.Item ref={ref} className={cn('shadcn-toggle-group-item', className)} {...props} data-slot="toggle-group-item" />;
});

ToggleGroup.displayName = 'ToggleGroup';
ToggleGroupItem.displayName = 'ToggleGroupItem';
