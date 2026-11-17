import { forwardRef, type ChangeEvent, type InputHTMLAttributes } from 'react';
import './shadcn.css';
import { cn } from './utils';

export interface SliderProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'type' | 'value' | 'defaultValue' | 'onChange'> {
  /** Supports the standard shadcn/Radix single-thumb array API. */
  value?: number | number[];
  defaultValue?: number | number[];
  onValueChange?: (value: number[]) => void;
  /** Accessible name for the native range thumb. */
  label?: string;
  onChange?: (event: ChangeEvent<HTMLInputElement>) => void;
}

/** Accessible native range input with the controlled API used by shadcn's Radix Slider. */
export const Slider = forwardRef<HTMLInputElement, SliderProps>(function Slider(
  { className, value, defaultValue, onValueChange, onChange, label, min = 0, max = 100, step = 1, 'aria-label': ariaLabel, ...props },
  ref,
) {
  const selectedValue = Array.isArray(value) ? value[0] : value;
  const initialValue = Array.isArray(defaultValue) ? defaultValue[0] : defaultValue;

  return (
    <input
      ref={ref}
      type="range"
      min={min}
      max={max}
      step={step}
      value={selectedValue}
      defaultValue={selectedValue === undefined ? initialValue : undefined}
      aria-label={label ?? ariaLabel}
      className={cn('mh-shadcn-theme', 'shadcn-slider', className)}
      onChange={event => {
        onChange?.(event);
        onValueChange?.([Number(event.currentTarget.value)]);
      }}
      {...props}
      data-slot="slider"
    />
  );
});

Slider.displayName = 'Slider';
