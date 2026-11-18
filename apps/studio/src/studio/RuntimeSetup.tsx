import { CheckIcon } from '@phosphor-icons/react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Label } from '@/components/ui/label';
import { runtimeSetupStage } from './workbench-state';
import type { RuntimeInfo } from './types';

type ModelBackend = RuntimeInfo['model_backends'][number];

interface RuntimeSetupProps {
  backend: ModelBackend;
  disabled: boolean;
  acknowledged: boolean;
  onAcknowledge: (checked: boolean) => void;
  onSetup: () => void;
  onDownload: () => void;
  onConvert: () => void;
}

export function RuntimeSetup({ backend, disabled, acknowledged, onAcknowledge, onSetup, onDownload, onConvert }: RuntimeSetupProps) {
  const stage = runtimeSetupStage(backend);
  const cuda = backend.id === 'pixal3d-cuda';
  if (stage === 'unsupported') {
    return <p className="text-muted-foreground">{cuda
      ? 'CUDA requires an NVIDIA GPU on Linux x86_64. It cannot run on this computer.'
      : 'Apple MLX requires an Apple Silicon Mac.'}</p>;
  }

  return <section className="grid gap-4" aria-label="Image-to-3D setup">
    <ol className="divide-y border-y">
      <li className="flex items-center gap-2 py-2">
        <Badge variant={stage === 'environment' ? 'outline' : 'secondary'} aria-hidden="true">{stage !== 'environment' ? <CheckIcon /> : '1'}</Badge>
        <span>Python environment</span>
        <span className="ml-auto text-muted-foreground">{stage === 'environment' ? 'Not installed' : 'Installed'}</span>
      </li>
      <li className="flex items-center gap-2 py-2">
        <Badge variant={stage === 'ready' ? 'secondary' : 'outline'} aria-hidden="true">{stage === 'ready' ? <CheckIcon /> : '2'}</Badge>
        <span>Model files</span>
        <span className="ml-auto text-muted-foreground">{stage === 'ready' ? 'Available' : stage === 'conversion' ? 'Needs conversion' : stage === 'incomplete' ? 'Check required' : 'Not installed'}</span>
      </li>
    </ol>
    {stage === 'environment' && <>
      <p className="text-muted-foreground">MeshHarbor installs uv if needed and creates a private Python 3.12 environment. {cuda ? 'It installs the CUDA profile and checks GPU compatibility. ' : 'It installs the locked MLX dependencies. '}No model files are downloaded in this step.</p>
      <Button disabled={disabled} onClick={onSetup}>Set up {cuda ? 'CUDA' : 'Python'} environment</Button>
    </>}
    {stage === 'models' && <>
      <div className="grid gap-2">
        <p className="text-muted-foreground">Obtain gated DINOv3 access, then run <code>{cuda ? 'meshharbor runtime cuda-hf-login' : 'meshharbor runtime hf-login'}</code> in a local terminal using the same data directory. Studio never asks for a token.</p>
        <p className="text-muted-foreground">Review the model and checkpoint terms before downloading. {cuda ? 'The files are about 25–30 GB; allow around 40 GiB free.' : 'The files are about 24 GB. Setup then converts the NAF checkpoint locally.'}</p>
        <div className="flex items-start gap-2">
          <Checkbox id="model-download-consent" checked={acknowledged} onCheckedChange={value => onAcknowledge(value === true)} disabled={disabled || !backend.downloads_enabled} />
          <Label htmlFor="model-download-consent" className="leading-relaxed">I reviewed the model terms, have gated access, and approve this download.</Label>
        </div>
      </div>
      <Button disabled={disabled || !acknowledged || !backend.downloads_enabled} onClick={onDownload}>Download model files</Button>
    </>}
    {stage === 'conversion' && <>
      <p className="text-muted-foreground">The downloaded NAF checkpoint needs a one-time conversion in a separate CPU-only environment.</p>
      <Button disabled={disabled || !backend.conversion_enabled} onClick={onConvert}>Finish NAF conversion</Button>
    </>}
    {stage === 'incomplete' && <p className="text-muted-foreground">The files are present, but the runtime checks have not passed. Run <code>meshharbor doctor</code> for details.</p>}
    {stage === 'ready' && <p className="text-muted-foreground">The local checks passed. Open a PNG or JPEG to generate a model.</p>}
  </section>;
}
