import { forwardRef, type ChangeEvent, type InputHTMLAttributes, type MutableRefObject } from 'react';
import './shadcn.css';
import { cn } from './utils';

export interface CheckboxProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'type' | 'onChange'> {
  onCheckedChange?: (checked: boolean) => void;
  onChange?: (event: ChangeEvent<HTMLInputElement>) => void;
}

export const Checkbox = forwardRef<HTMLInputElement, CheckboxProps>(function Checkbox(
  { className, checked, defaultChecked, onCheckedChange, onChange, ...props },
  forwardedRef,
) {
  return (
    <input
      ref={node => {
        if (typeof forwardedRef === 'function') forwardedRef(node);
        else if (forwardedRef) (forwardedRef as MutableRefObject<HTMLInputElement | null>).current = node;
      }}
      type="checkbox"
      checked={checked}
      defaultChecked={defaultChecked}
      onChange={event => {
        onChange?.(event);
        onCheckedChange?.(event.currentTarget.checked);
      }}
      className={cn('mh-shadcn-theme', 'shadcn-checkbox', className)}
      {...props}
      data-slot="checkbox"
    />
  );
});

Checkbox.displayName = 'Checkbox';
