import { forwardRef, type SelectHTMLAttributes, type OptionHTMLAttributes, type OptgroupHTMLAttributes } from 'react';
import './shadcn.css';
import { cn } from './utils';

/** Styled native select. Keeping actual <option>/<optgroup> children preserves native semantics. */
export const NativeSelect = forwardRef<HTMLSelectElement, SelectHTMLAttributes<HTMLSelectElement>>(
  function NativeSelect({ className, children, ...props }, ref) {
    return <select ref={ref} className={cn('mh-shadcn-theme', 'shadcn-native-select', className)} {...props} data-slot="native-select">{children}</select>;
  },
);

export const NativeSelectOption = forwardRef<HTMLOptionElement, OptionHTMLAttributes<HTMLOptionElement>>(
  function NativeSelectOption({ className, ...props }, ref) {
    return <option ref={ref} className={className} {...props} />;
  },
);

export const NativeSelectOptGroup = forwardRef<HTMLOptGroupElement, OptgroupHTMLAttributes<HTMLOptGroupElement>>(
  function NativeSelectOptGroup({ className, ...props }, ref) {
    return <optgroup ref={ref} className={className} {...props} />;
  },
);

NativeSelect.displayName = 'NativeSelect';
NativeSelectOption.displayName = 'NativeSelectOption';
NativeSelectOptGroup.displayName = 'NativeSelectOptGroup';
