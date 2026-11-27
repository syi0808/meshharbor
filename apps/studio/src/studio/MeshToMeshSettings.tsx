import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';

export function MeshToMeshSettings({ backends, disabled, running, sourceTriangles, onRun, onSetup }: {
  backends: { id: string; label: string; ready: boolean }[];
  disabled: boolean;
  running: boolean;
  sourceTriangles: number;
  onRun: (parameters: Record<string, number | string>) => void;
  onSetup: () => void;
}) {
  const [targetVertices, setTargetVertices] = useState('2000');
  const [maxTriangles, setMaxTriangles] = useState('');
  const [atlasResolution, setAtlasResolution] = useState(1024);
  const [chosenBackend, setChosenBackend] = useState('');
  const backend = backends.find(item => item.id === chosenBackend) ?? backends.find(item => item.ready) ?? backends[0];
  const vertices = Number(targetVertices);
  const limit = Number(maxTriangles);
  const maxLimit = sourceTriangles > 0 ? Math.min(20_000, sourceTriangles - 1) : 20_000;
  const verticesValid = Number.isInteger(vertices) && vertices >= 200 && vertices <= 5000;
  const hasLimit = maxTriangles.trim() !== '';
  const limitValid = !hasLimit || Number.isInteger(limit) && limit >= 4 && limit <= maxLimit;

  return <div className="grid gap-3">
    <Field>
      <FieldLabel htmlFor="mesh-to-mesh-model">Mesh to Mesh model</FieldLabel>
      <NativeSelect id="mesh-to-mesh-model" className="w-full" value={backend?.id ?? ''} disabled={running || !backend}
        onChange={event => setChosenBackend(event.currentTarget.value)}>
        {!backend && <NativeSelectOption value="">LATO.2</NativeSelectOption>}
        {backends.map(item => <NativeSelectOption key={item.id} value={item.id}>
          LATO.2 · {item.label}{item.ready ? '' : ' (not ready)'}
        </NativeSelectOption>)}
      </NativeSelect>
    </Field>
    {!backend?.ready && <>
      <p className="text-muted-foreground">Install a supported LATO.2 backend to run Mesh to Mesh.</p>
      <Button variant="outline" size="sm" onClick={onSetup}>View model setup</Button>
    </>}
    <Field>
      <FieldLabel htmlFor="lato2-target-vertices">Target vertices</FieldLabel>
      <Input id="lato2-target-vertices" type="number" min={200} max={5000} step={1} value={targetVertices}
        aria-invalid={!verticesValid} disabled={running} onChange={event => setTargetVertices(event.currentTarget.value)} />
      <FieldDescription>Approximate. The exported vertex count can increase at UV seams.</FieldDescription>
    </Field>
    <Field>
      <FieldLabel htmlFor="lato2-max-triangles">Maximum triangles (optional)</FieldLabel>
      <Input id="lato2-max-triangles" type="number" min={4} max={maxLimit} step={1} value={maxTriangles}
        aria-invalid={!limitValid} disabled={running} onChange={event => setMaxTriangles(event.currentTarget.value)} />
      <FieldDescription>The job fails without saving if the new mesh exceeds this limit.</FieldDescription>
    </Field>
    <Field>
      <FieldLabel htmlFor="lato2-atlas-resolution">Texture atlas size</FieldLabel>
      <NativeSelect id="lato2-atlas-resolution" className="w-full" value={atlasResolution} disabled={running}
        onChange={event => setAtlasResolution(Number(event.currentTarget.value))}>
        {[512, 1024, 2048].map(size => <NativeSelectOption key={size} value={size}>{size} × {size}</NativeSelectOption>)}
      </NativeSelect>
    </Field>
    {!running && <Button className="w-full" disabled={disabled || !backend?.ready || !verticesValid || !limitValid || sourceTriangles <= 4}
      onClick={() => { if (backend) onRun({ backend_id: backend.id, target_vertices: vertices, atlas_resolution: atlasResolution, ...(hasLimit ? { max_triangles: limit } : {}) }); }}>
      Run Mesh to Mesh
    </Button>}
  </div>;
}
