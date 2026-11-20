import { ImageSquareIcon } from '@phosphor-icons/react';
import { useEffect, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Field, FieldError, FieldGroup, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Skeleton } from '@/components/ui/skeleton';
import { HelpTip } from './HelpTip';
import { STUDIO_IMAGE_DRAG_TYPE } from './reference-drag';
import type { Asset, OrbitCamera, PosedGenerationInput, PosedViewMode } from './types';

type CameraDraft = Record<keyof OrbitCamera, string>;
export type ViewDraft = { image: Asset | null; camera: CameraDraft };
type LoadReference = (image: Asset, signal: AbortSignal) => Promise<Blob>;
const cameraFields = [
  { id: 'azimuth_degrees', label: 'Azimuth (°)', min: -360, max: 360 },
  { id: 'elevation_degrees', label: 'Elevation (°)', min: -90, max: 90 },
  { id: 'distance', label: 'Distance', min: 0.01, max: 1000 },
  { id: 'fov_degrees', label: 'FOV (°)', min: 1, max: 179 },
] as const;
const presetLabels = ['Front', 'Right', 'Back', 'Left', 'Top', 'Bottom'];

export function newView(index: number): ViewDraft {
  return {
    image: null,
    camera: {
      azimuth_degrees: String(index * 45),
      elevation_degrees: '0',
      distance: '3.119',
      fov_degrees: '20',
    },
  };
}

export function defaultViews(mode: PosedViewMode): ViewDraft[] {
  return Array.from({ length: mode === 'four' ? 4 : mode === 'six' ? 6 : 2 }, (_, index) => newView(index));
}

function readCameraNumber(input: string, min: number, max: number) {
  const number = Number(input);
  return input.trim() !== '' && Number.isFinite(number) && number >= min && number <= max ? number : null;
}

export function buildPosedInput(mode: PosedViewMode, views: ViewDraft[], maxViews: number): PosedGenerationInput | null {
  if (views.length > maxViews || (mode === 'custom' ? views.length < 2 : views.length !== defaultViews(mode).length)) return null;
  const references: PosedGenerationInput['references'] = [];
  const ids = new Set<string>();
  for (const view of views) {
    if (!view.image || ids.has(view.image.id)) return null;
    ids.add(view.image.id);
    if (mode === 'custom') {
      const azimuth = readCameraNumber(view.camera.azimuth_degrees, -360, 360);
      const elevation = readCameraNumber(view.camera.elevation_degrees, -90, 90);
      const distance = readCameraNumber(view.camera.distance, 0.01, 1000);
      const fov = readCameraNumber(view.camera.fov_degrees, 1, 179);
      if (azimuth === null || elevation === null || distance === null || fov === null) return null;
      references.push({ asset_id: view.image.id, camera: { azimuth_degrees: azimuth, elevation_degrees: elevation, distance, fov_degrees: fov } });
    } else {
      references.push({ asset_id: view.image.id });
    }
  }
  return { mode, references };
}

function ReferencePreview({ image, load }: { image: Asset; load: LoadReference }) {
  const [preview, setPreview] = useState<{ id: string; url: string | null }>();
  useEffect(() => {
    const controller = new AbortController();
    let url: string | undefined;
    void load(image, controller.signal).then(blob => {
      if (controller.signal.aborted) return;
      url = URL.createObjectURL(blob);
      setPreview({ id: image.id, url });
    }).catch(() => {
      if (!controller.signal.aborted) setPreview({ id: image.id, url: null });
    });
    return () => {
      controller.abort();
      if (url) URL.revokeObjectURL(url);
    };
  }, [image, load]);

  return <figure className="grid min-w-0 gap-2">
    {preview?.id !== image.id ? <Skeleton className="h-24 w-full" aria-label="Loading reference preview" />
      : preview.url ? <img src={preview.url} alt={`Reference preview: ${image.name}`} decoding="async"
        className="max-h-48 w-full rounded-md border bg-muted object-contain" />
        : <p className="text-muted-foreground">Preview unavailable.</p>}
    <figcaption className="truncate text-muted-foreground" title={image.name}>{image.name}</figcaption>
  </figure>;
}

function ReferencePicker({ image, load, disabled, name, onPick, onDropImage, onClear }: {
  image: Asset | null; load: LoadReference; disabled: boolean; name?: string;
  onPick: (files: FileList) => void; onClear: () => void;
  onDropImage: (id: string) => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const [over, setOver] = useState(false);
  const action = image ? 'Replace reference' : 'Choose reference';
  return <div className="grid min-w-0 gap-2 rounded-md border border-dashed border-muted-foreground/40 p-2 data-[over=true]:border-primary data-[over=true]:bg-accent"
    data-over={over} role="group" aria-label={name ? `Drop image for ${name}` : 'Drop a reference image'}
    onDragOver={event => {
      if (disabled || !event.dataTransfer.types.includes(STUDIO_IMAGE_DRAG_TYPE)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = 'copy';
      setOver(true);
    }} onDragLeave={event => {
      if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setOver(false);
    }} onDrop={event => {
      if (!event.dataTransfer.types.includes(STUDIO_IMAGE_DRAG_TYPE)) return;
      event.preventDefault();
      event.stopPropagation();
      setOver(false);
      if (!disabled) onDropImage(event.dataTransfer.getData(STUDIO_IMAGE_DRAG_TYPE));
    }}>
    {image && <ReferencePreview image={image} load={load} />}
    <div className="flex flex-wrap gap-2">
      <Input ref={input} type="file" hidden className="hidden" accept="image/png,image/jpeg,.png,.jpg,.jpeg"
        onChange={event => {
          if (event.currentTarget.files?.length) onPick(event.currentTarget.files);
          event.currentTarget.value = '';
        }} />
      <Button variant="outline" size="sm" aria-label={name ? `${action}: ${name}` : undefined} disabled={disabled} onClick={() => input.current?.click()}>
        <ImageSquareIcon data-icon="inline-start" aria-hidden="true" />{action}
      </Button>
      {image && <Button variant="ghost" size="sm" disabled={disabled} onClick={onClear}>Clear</Button>}
    </div>
  </div>;
}

export function SingleReference({ image, load, disabled, onPick, onDropImage, onClear }: {
  image: Asset | null; load: LoadReference; disabled: boolean; onPick: (files: FileList) => void; onClear: () => void;
  onDropImage: (id: string) => void;
}) {
  return <Card size="sm" role="region" aria-label="Reference image" className="min-w-0 gap-2">
    <CardHeader>
      <CardTitle className="flex items-center gap-1 text-xs"><h3>Reference image</h3>
        <HelpTip label="About reference images">Choose a file or drag an image from Inventory. This keeps the current viewport selection.</HelpTip>
      </CardTitle>
      {!image && <CardDescription>PNG or JPEG</CardDescription>}
    </CardHeader>
    <CardContent><ReferencePicker image={image} load={load} disabled={disabled} onPick={onPick} onDropImage={onDropImage} onClear={onClear} /></CardContent>
  </Card>;
}

export function PosedReferences({ mode, views, maxViews, load, disabled, onPick, onDropImage, onChange }: {
  mode: PosedViewMode; views: ViewDraft[]; maxViews: number; load: LoadReference; disabled: boolean;
  onPick: (files: FileList, index?: number) => void; onChange: (views: ViewDraft[]) => void;
  onDropImage: (id: string, index: number) => void;
}) {
  const bulkInput = useRef<HTMLInputElement>(null);
  const update = (index: number, view: ViewDraft) => onChange(views.map((item, itemIndex) => itemIndex === index ? view : item));
  const custom = mode === 'custom';
  const hasDuplicate = new Set(views.flatMap(view => view.image ? [view.image.id] : [])).size < views.filter(view => view.image).length;
  return <section className="grid min-w-0 gap-3" aria-label="Reference views">
    <div className="flex items-center gap-1">
      <h3 className="font-medium">Reference views</h3>
      <HelpTip label="About reference views">
        {custom ? 'Start with the front view. Set the camera for each image.'
          : 'Match each image to its direction. Presets use the same camera distance and a 20° field of view.'}{' '}
        Masked PNGs with consistent framing work best.
      </HelpTip>
    </div>
    {custom && <p className="text-muted-foreground">2–{maxViews} images</p>}
    <Input ref={bulkInput} type="file" hidden className="hidden" multiple accept="image/png,image/jpeg,.png,.jpg,.jpeg" onChange={event => {
      if (event.currentTarget.files?.length) onPick(event.currentTarget.files);
      event.currentTarget.value = '';
    }} />
    {custom && <Button variant="outline" size="sm" className="justify-self-start" disabled={disabled}
      onClick={() => bulkInput.current?.click()}>Choose multiple images</Button>}
    {views.map((view, index) => {
      const label = custom ? `View ${index + 1}` : presetLabels[index];
      return <FieldSet className="min-w-0 items-start gap-3 rounded-md border p-3" key={index}>
        <FieldLegend variant="label" className="mb-0 px-1">{label}</FieldLegend>
        <FieldGroup className="gap-3">
          <ReferencePicker image={view.image} name={label} load={load} disabled={disabled}
            onPick={files => onPick(files, index)} onDropImage={id => onDropImage(id, index)} onClear={() => update(index, { ...view, image: null })} />
          {custom && <FieldGroup className="grid grid-cols-2 gap-2">
            {cameraFields.map(field => {
              const id = `reference-${index}-${field.id}`;
              const invalid = readCameraNumber(view.camera[field.id], field.min, field.max) === null;
              return <Field key={field.id} data-invalid={invalid || undefined}>
                <FieldLabel htmlFor={id}>{field.label}</FieldLabel>
                <Input id={id} type="number" step="any" min={field.min} max={field.max} value={view.camera[field.id]}
                  aria-invalid={invalid} disabled={disabled}
                  onChange={event => update(index, { ...view, camera: { ...view.camera, [field.id]: event.target.value } })} />
              </Field>;
            })}
          </FieldGroup>}
          {custom && views.length > 2 && <Button variant="ghost" size="sm" className="self-start" disabled={disabled}
            onClick={() => onChange(views.filter((_, itemIndex) => itemIndex !== index))}>Remove {label.toLowerCase()}</Button>}
        </FieldGroup>
      </FieldSet>;
    })}
    {hasDuplicate && <FieldError>Choose a different image for each view.</FieldError>}
    {custom && views.length < maxViews && <Button variant="outline" size="sm" className="justify-self-start" disabled={disabled}
      onClick={() => onChange([...views, newView(views.length)])}>Add view</Button>}
  </section>;
}
