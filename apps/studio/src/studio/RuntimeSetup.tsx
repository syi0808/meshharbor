import { CheckIcon } from '@phosphor-icons/react';
import { Button } from '../ui/Button';
import { Checkbox } from '../ui/Checkbox';
import { Label } from '../ui/Label';
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
    return <p className="runtime-explanation">{cuda
      ? 'CUDA requires an NVIDIA GPU on Linux x86_64. It cannot run on this computer.'
      : 'Apple MLX requires an Apple Silicon Mac.'}</p>;
  }

  return <section className="runtime-steps" aria-label="Image-to-3D setup">
    <ol className="runtime-checklist">
      <li>
        <span className="runtime-step-marker" data-complete={stage !== 'environment'}>{stage !== 'environment' ? <CheckIcon aria-hidden="true" /> : '1'}</span>
        <span>Python environment</span>
        <span className="runtime-step-status">{stage === 'environment' ? 'Not installed' : 'Installed'}</span>
      </li>
      <li>
        <span className="runtime-step-marker" data-complete={stage === 'ready'}>{stage === 'ready' ? <CheckIcon aria-hidden="true" /> : '2'}</span>
        <span>Model files</span>
        <span className="runtime-step-status">{stage === 'ready' ? 'Available' : stage === 'conversion' ? 'Needs conversion' : stage === 'incomplete' ? 'Check required' : 'Not installed'}</span>
      </li>
    </ol>
    {stage === 'environment' && <>
      <p className="runtime-explanation">MeshHarbor installs uv if needed and creates a private Python 3.12 environment. {cuda ? 'It installs the CUDA profile and checks GPU compatibility. ' : 'It installs the locked MLX dependencies. '}No model files are downloaded in this step.</p>
      <Button className="runtime-step-action" disabled={disabled} onClick={onSetup}>Set up {cuda ? 'CUDA' : 'Python'} environment</Button>
    </>}
    {stage === 'models' && <>
      <div className="runtime-model-consent">
        <p className="runtime-explanation">Obtain gated DINOv3 access, then run <code>{cuda ? 'meshharbor runtime cuda-hf-login' : 'meshharbor runtime hf-login'}</code> in a local terminal using the same data directory. Studio never asks for a token.</p>
        <p className="runtime-explanation">Review the model and checkpoint terms before downloading. {cuda ? 'The files are about 25–30 GB; allow around 40 GiB free.' : 'The files are about 24 GB. Setup then converts the NAF checkpoint locally.'}</p>
        <Label className="runtime-download-consent">
          <Checkbox checked={acknowledged} onCheckedChange={value => onAcknowledge(value === true)} disabled={disabled || !backend.downloads_enabled} />
          <span>I reviewed the model terms, have gated access, and approve this download.</span>
        </Label>
      </div>
      <Button className="runtime-step-action" disabled={disabled || !acknowledged || !backend.downloads_enabled} onClick={onDownload}>Download model files</Button>
    </>}
    {stage === 'conversion' && <>
      <p className="runtime-explanation">The downloaded NAF checkpoint needs a one-time conversion in a separate CPU-only environment.</p>
      <Button className="runtime-step-action" disabled={disabled || !backend.conversion_enabled} onClick={onConvert}>Finish NAF conversion</Button>
    </>}
    {stage === 'incomplete' && <p className="runtime-explanation">The files are present, but the runtime checks have not passed. Run <code>meshharbor doctor</code> for details.</p>}
    {stage === 'ready' && <p className="runtime-explanation">The local checks passed. Open a PNG or JPEG to generate a model.</p>}
  </section>;
}
