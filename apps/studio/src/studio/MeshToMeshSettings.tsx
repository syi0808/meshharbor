import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { preferredModelBackend } from './model-backends';
import type { RuntimeInfo } from './types';

type MeshToMeshModel = { id: string; label: string; backendId: string; ready: boolean };

export function selectableMeshToMeshModels(runtime: RuntimeInfo | null): MeshToMeshModel[] {
  if (!runtime) return [];
  return (runtime.model_catalog ?? []).flatMap(model => {
    const backends = (runtime.model_backends ?? []).filter(backend =>
      model.backend_ids.includes(backend.id) && backend.mesh_to_mesh_ready !== undefined);
    const selected = preferredModelBackend(backends, runtime.platform, backend => backend.mesh_to_mesh_ready === true);
    return selected ? [{
      id: model.id,
      label: model.title,
      backendId: selected.id,
      ready: selected.mesh_to_mesh_ready === true,
    }] : [];
  });
}

export function MeshToMeshSettings({ models, disabled, running, onRun, onSetup }: {
  models: MeshToMeshModel[];
  disabled: boolean;
  running: boolean;
  onRun: (parameters: Record<string, number | string>) => void;
  onSetup: () => void;
}) {
  const [targetTriangles, setTargetTriangles] = useState('4000');
  const [atlasResolution, setAtlasResolution] = useState(1024);
  const [modelId, setModelId] = useState('');
  const model = models.find(item => item.id === modelId) ?? models.find(item => item.ready) ?? models[0];
  const triangles = Number(targetTriangles);
  const trianglesValid = Number.isSafeInteger(triangles) && triangles >= 4;

  return <div className="grid gap-3">
    <Field>
      <FieldLabel htmlFor="mesh-to-mesh-model">Mesh to Mesh model</FieldLabel>
      <NativeSelect id="mesh-to-mesh-model" className="w-full" value={model?.id ?? ''} disabled={running || !model}
        onChange={event => setModelId(event.currentTarget.value)}>
        {!model && <NativeSelectOption value="">No models available</NativeSelectOption>}
        {models.map(item => <NativeSelectOption key={item.id} value={item.id}>
          {item.label}{item.ready ? '' : ' (setup required)'}
        </NativeSelectOption>)}
      </NativeSelect>
    </Field>
    {!model?.ready && <>
      <p role="status" className="text-muted-foreground">Install a supported model to run Mesh to Mesh.</p>
      <Button variant="outline" size="sm" onClick={onSetup}>View model setup</Button>
    </>}
    <Field>
      <FieldLabel htmlFor="lato2-target-triangles">Target triangles</FieldLabel>
      <Input id="lato2-target-triangles" type="number" min={4} step={1} value={targetTriangles}
        aria-invalid={!trianglesValid} disabled={running} onChange={event => setTargetTriangles(event.currentTarget.value)} />
      <FieldDescription>Approximate, not a limit. The result may have more or fewer triangles.</FieldDescription>
    </Field>
    <Field>
      <FieldLabel htmlFor="lato2-atlas-resolution">Texture atlas size</FieldLabel>
      <NativeSelect id="lato2-atlas-resolution" className="w-full" value={atlasResolution} disabled={running}
        onChange={event => setAtlasResolution(Number(event.currentTarget.value))}>
        {[512, 1024, 2048].map(size => <NativeSelectOption key={size} value={size}>{size} × {size}</NativeSelectOption>)}
      </NativeSelect>
    </Field>
    {!running && <Button className="w-full" disabled={disabled || !model?.ready || !trianglesValid}
      onClick={() => { if (model?.ready) onRun({ backend_id: model.backendId, target_triangles: triangles, atlas_resolution: atlasResolution }); }}>
      Run Mesh to Mesh
    </Button>}
  </div>;
}
