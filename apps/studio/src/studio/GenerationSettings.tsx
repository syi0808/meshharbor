import { Field, FieldDescription, FieldError, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import type { GenerationParameter, GenerationProfile, RuntimeInfo } from './types';

type Draft = Record<string, string>;

export function readyGenerationProfiles(runtime: RuntimeInfo | null): GenerationProfile[] {
  return (runtime?.model_catalog ?? []).flatMap(model => model.backend_ids.flatMap(id => {
    const backend = runtime?.model_backends.find(item => item.id === id);
    return backend?.ready && backend.generation?.id === id
      ? [{ ...backend.generation, label: `${model.title} · ${backend.generation.label}` }]
      : [];
  }));
}

function parameterValue(parameter: GenerationParameter, draft: Draft): string {
  return parameter.kind === 'fixed' ? String(parameter.default) : draft[parameter.id] ?? String(parameter.default);
}

function maximum(parameter: Extract<GenerationParameter, { kind: 'integer' }>): number {
  return Math.min(parameter.max ?? Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER);
}

function parseParameter(parameter: GenerationParameter, input: string): number | null {
  if (!/^[0-9]+$/.test(input)) return null;
  const value = Number(input);
  if (!Number.isSafeInteger(value)) return null;
  if (parameter.kind === 'fixed' && value === parameter.default) return value;
  if (parameter.kind === 'choice' && parameter.values.includes(value)) return value;
  if (parameter.kind === 'integer' && value >= parameter.min && value <= maximum(parameter)) return value;
  return null;
}

export function generationParameters(profile: GenerationProfile, draft: Draft): Record<string, number> | null {
  const entries: [string, number][] = [];
  for (const parameter of profile.parameters) {
    const value = parseParameter(parameter, parameterValue(parameter, draft));
    if (value === null) return null;
    entries.push([parameter.id, value]);
  }
  return Object.fromEntries(entries);
}

export function GenerationParameterFields({ profile, draft, disabled, onChange }: {
  profile: GenerationProfile;
  draft: Draft;
  disabled: boolean;
  onChange: (id: string, value: string) => void;
}) {
  return <div className="grid gap-4">
    {profile.parameters.map(parameter => {
      const id = `generation-parameter-${parameter.id}`;
      const value = parameterValue(parameter, draft);
      const invalid = parseParameter(parameter, value) === null;
      const describedBy = `${id}-description${invalid ? ` ${id}-error` : ''}`;
      return <Field key={parameter.id} data-invalid={invalid || undefined}>
        <FieldLabel htmlFor={id}>{parameter.label}</FieldLabel>
        {parameter.kind === 'choice' ? <NativeSelect id={id} className="w-full" value={value} disabled={disabled}
          aria-invalid={invalid} aria-describedby={describedBy} onChange={event => onChange(parameter.id, event.target.value)}>
          {parameter.values.map(option => <NativeSelectOption key={option} value={option}>{option}</NativeSelectOption>)}
        </NativeSelect> : <Input id={id} type="number" value={value} disabled={disabled}
          readOnly={parameter.kind === 'fixed'} min={parameter.kind === 'integer' ? parameter.min : undefined}
          max={parameter.kind === 'integer' ? maximum(parameter) : undefined} step={1}
          aria-invalid={invalid} aria-describedby={describedBy} onChange={event => onChange(parameter.id, event.target.value)} />}
        <FieldDescription id={`${id}-description`}>{parameter.description}</FieldDescription>
        {invalid && <FieldError id={`${id}-error`}>{parameter.kind === 'integer'
          ? `Enter a whole number from ${parameter.min.toLocaleString()} to ${maximum(parameter).toLocaleString()}.`
          : 'Choose a supported value.'}</FieldError>}
      </Field>;
    })}
  </div>;
}
