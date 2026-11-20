import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from '@/components/ui/accordion';
import { Badge } from '@/components/ui/badge';
import { Checkbox } from '@/components/ui/checkbox';
import { Field, FieldError, FieldGroup, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { HelpTip } from './HelpTip';
import type { GenerationModel, GenerationParameter, GenerationParameters, GenerationTuning, RuntimeInfo } from './types';

type Draft = Record<string, string>;

export function readyGenerationProfiles(runtime: RuntimeInfo | null): GenerationModel[] {
  const preferred = runtime?.platform === 'linux' ? 'pixal3d-cuda' : 'pixal3d-mlx';
  return (runtime?.model_catalog ?? []).flatMap(model => {
    const backends = (runtime?.model_backends ?? []).filter(backend =>
      model.backend_ids.includes(backend.id) && backend.ready && backend.generation?.id === backend.id);
    const selected = backends.find(backend => backend.id === preferred) ?? backends[0];
    const profile = selected?.generation;
    if (!profile) return [];
    const modes = profile.input_modes === undefined ? (['image'] as const) : profile.input_modes;
    if (!modes.length) return [];
    return [{
      ...profile,
      modelId: model.id,
      label: model.title,
      input_modes: [...modes],
      view_modes: profile.view_modes ?? ['single'],
      max_views: profile.max_views ?? 1,
      multiviewReady: selected.multiview_ready === true,
    }];
  });
}

function parameterValue(parameter: GenerationParameter, draft: Draft): string {
  return parameter.kind === 'fixed' ? String(parameter.default) : draft[parameter.id] ?? String(parameter.default);
}

function parseParameter(parameter: GenerationParameter, input: string): number | string | boolean | null {
  if (parameter.kind === 'choice') return parameter.values.find(value => String(value) === input) ?? null;
  if (parameter.kind === 'boolean') return input === 'true' ? true : input === 'false' ? false : null;
  if (input.trim() === '') return null;
  const value = Number(input);
  if (parameter.kind === 'fixed') return value === parameter.default ? value : null;
  if (parameter.kind === 'integer' && !Number.isSafeInteger(value)) return null;
  if (parameter.id === 'max_faces' && value > 0 && value < 4) return null;
  return Number.isFinite(value) && value >= parameter.min && value <= (parameter.max ?? Number.MAX_SAFE_INTEGER) ? value : null;
}

function parametersFor(profile: GenerationModel, posed: boolean) {
  return profile.parameters.filter(parameter => !posed || !parameter.single_view_only);
}

function hasInvalidInterval(parameter: GenerationParameter, value: string, parameters: GenerationParameter[], draft: Draft): boolean {
  if (!parameter.id.endsWith('guidance_start')) return false;
  const end = parameters.find(candidate => candidate.id === parameter.id.replace(/guidance_start$/, 'guidance_end'));
  return end !== undefined && Number(value) > Number(parameterValue(end, draft));
}

export function generationParameters(profile: GenerationModel, draft: Draft, posed: boolean): GenerationParameters | null {
  const tuning: GenerationTuning = {};
  const core: Record<string, number> = {};
  for (const parameter of parametersFor(profile, posed)) {
    const value = parseParameter(parameter, parameterValue(parameter, draft));
    if (value === null) return null;
    if (parameter.tuning) tuning[parameter.id] = value;
    else if (typeof value === 'number') core[parameter.id] = value;
  }
  for (const stage of ['structure', 'shape', 'texture']) {
    const start = tuning[`${stage}_guidance_start`];
    const end = tuning[`${stage}_guidance_end`];
    if (typeof start === 'number' && typeof end === 'number' && start > end) return null;
  }
  return { ...core, ...(Object.keys(tuning).length ? { tuning } : {}) };
}

export function GenerationParameterFields({ profile, draft, posed, disabled, onChange }: {
  profile: GenerationModel;
  draft: Draft;
  posed: boolean;
  disabled: boolean;
  onChange: (id: string, value: string) => void;
}) {
  const sections = new Map<string, GenerationParameter[]>();
  for (const parameter of parametersFor(profile, posed)) {
    const group = parameter.section ?? 'Output';
    sections.set(group, [...(sections.get(group) ?? []), parameter]);
  }
  return <Accordion key={profile.id} type="multiple" defaultValue={['Output', 'Sampling']} className="min-w-0">
    {[...sections].map(([section, parameters]) => <AccordionItem key={section} value={section}>
      <AccordionTrigger className="items-center gap-2 focus-visible:border-ring">
        <span>{section}</span>
        {parameters.some(parameter => {
          const value = parameterValue(parameter, draft);
          return parseParameter(parameter, value) === null || hasInvalidInterval(parameter, value, profile.parameters, draft);
        }) && <Badge variant="destructive">Check values</Badge>}
      </AccordionTrigger>
      <AccordionContent className="px-1">
      <FieldSet className="min-w-0">
      <FieldLegend variant="label" className="sr-only">{section}</FieldLegend>
      <FieldGroup className="gap-3">
        {parameters.map(parameter => {
          const id = `generation-parameter-${parameter.id}`;
          const value = parameterValue(parameter, draft);
          const invalid = parseParameter(parameter, value) === null;
          const invalidInterval = hasInvalidInterval(parameter, value, profile.parameters, draft);
          const describedBy = invalid || invalidInterval ? `${id}-error` : undefined;
          const label = <>
            <FieldLabel htmlFor={id}>{parameter.label}</FieldLabel>
            {parameter.description && <HelpTip label={`About ${parameter.label}`}>{parameter.description}</HelpTip>}
          </>;
          return <Field key={parameter.id} data-invalid={invalid || invalidInterval || undefined}>
            {parameter.kind === 'boolean' ? <div className="flex items-center gap-2">
              <Checkbox id={id} checked={value === 'true'} disabled={disabled} aria-describedby={describedBy}
                onCheckedChange={checked => onChange(parameter.id, String(checked === true))} />
              {label}
            </div> : <>
              <div className="flex items-center gap-1">{label}</div>
              {parameter.kind === 'choice' ? <NativeSelect id={id} className="w-full" value={value} disabled={disabled}
                aria-invalid={invalid} aria-describedby={describedBy} onChange={event => onChange(parameter.id, event.target.value)}>
                {parameter.values.map(option => <NativeSelectOption key={option} value={option}>{option}</NativeSelectOption>)}
              </NativeSelect> : <Input id={id} type="number" value={value} disabled={disabled}
                readOnly={parameter.kind === 'fixed'} min={parameter.kind === 'fixed' ? undefined : parameter.min}
                max={parameter.kind === 'fixed' ? undefined : parameter.max ?? Number.MAX_SAFE_INTEGER}
                step={parameter.kind === 'number' ? 'any' : 1}
                aria-invalid={invalid || invalidInterval} aria-describedby={describedBy} onChange={event => onChange(parameter.id, event.target.value)} />}
            </>}
            {(invalid || invalidInterval) && <FieldError id={`${id}-error`}>{invalidInterval ? 'Start must not exceed end.'
              : parameter.kind === 'integer' || parameter.kind === 'number'
                ? `Enter ${parameter.kind === 'integer' ? 'a whole number' : 'a number'} from ${parameter.min.toLocaleString()} to ${(parameter.max ?? Number.MAX_SAFE_INTEGER).toLocaleString()}.`
                : 'Choose a supported value.'}</FieldError>}
          </Field>;
        })}
      </FieldGroup>
      </FieldSet>
      </AccordionContent>
    </AccordionItem>)}
  </Accordion>;
}
