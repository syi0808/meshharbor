import { CheckIcon } from '@phosphor-icons/react';
import { useState, type FormEvent } from 'react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { runtimeSetupStage } from './workbench-state';
import type { HuggingFaceAccess, RuntimeInfo } from './types';

type ModelBackend = RuntimeInfo['model_backends'][number];

interface RuntimeSetupProps {
  backend: ModelBackend;
  disabled: boolean;
  acknowledged: boolean;
  onAcknowledge: (checked: boolean) => void;
  onCheckAccess: (token?: string) => Promise<HuggingFaceAccess>;
  onSetup: () => void;
  onDownload: () => void;
  onConvert: () => void;
}

export function RuntimeSetup(props: RuntimeSetupProps) {
  const { backend, disabled, onSetup, onConvert } = props;
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
    {stage === 'models' && <ModelDownloadSetup {...props} />}
    {stage === 'conversion' && <>
      <p className="text-muted-foreground">The downloaded NAF checkpoint needs a one-time conversion in a separate CPU-only environment.</p>
      <Button disabled={disabled || !backend.conversion_enabled} onClick={onConvert}>Finish NAF conversion</Button>
    </>}
    {stage === 'incomplete' && <p className="text-muted-foreground">The files are present, but the runtime checks have not passed. Run <code>meshharbor doctor</code> for details.</p>}
    {stage === 'ready' && <p className="text-muted-foreground">The local checks passed. Open a PNG or JPEG to generate a model.</p>}
  </section>;
}

function ModelDownloadSetup({ backend, disabled, acknowledged, onAcknowledge, onCheckAccess, onDownload }: RuntimeSetupProps) {
  const [access, setAccess] = useState<HuggingFaceAccess | 'checking' | null>(null);
  const [error, setError] = useState('');
  const checking = access === 'checking';
  const downloadBlocked = disabled || access !== 'verified' || !backend.downloads_enabled;

  async function checkAccess(token?: string) {
    setAccess('checking');
    setError('');
    onAcknowledge(false);
    try {
      setAccess(await onCheckAccess(token));
    } catch (error) {
      setAccess(null);
      setError(error instanceof Error ? error.message : 'Could not check DINOv3 access.');
    }
  }

  function connectToken(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const token = new FormData(event.currentTarget).get('token');
    event.currentTarget.reset();
    if (typeof token === 'string') void checkAccess(token);
  }

  return <>
    <ol className="grid list-decimal gap-2 pl-5">
      <li>
        <a className="text-primary underline underline-offset-4" href="https://huggingface.co/facebook/dinov3-vitl16-pretrain-lvd1689m" target="_blank" rel="noreferrer">Review terms and request DINOv3 access</a>
        <p className="text-muted-foreground">Sign in to Hugging Face and submit the access request there.</p>
      </li>
      <li>
        <a className="text-primary underline underline-offset-4" href="https://huggingface.co/settings/tokens" target="_blank" rel="noreferrer">Create a Hugging Face token</a>
        <p className="text-muted-foreground">Use the same account. Give the token read access to public gated repositories; write access is not needed.</p>
      </li>
    </ol>
    <form className="grid gap-2" method="post" autoComplete="off" onSubmit={connectToken}>
      <Label htmlFor="huggingface-token">Hugging Face access token</Label>
      <Input id="huggingface-token" name="token" type="password" placeholder="hf_…" maxLength={512} required disabled={disabled || checking} autoComplete="off" />
      <p className="text-muted-foreground">MeshHarbor saves the token only in this backend’s local Hugging Face cache after verifying DINOv3 file access.</p>
      <div className="flex flex-wrap gap-2">
        <Button type="submit" variant="secondary" disabled={disabled || checking}>Connect token</Button>
        <Button type="button" variant="outline" disabled={disabled || checking} onClick={() => void checkAccess()}>Check saved token</Button>
      </div>
    </form>
    {access && <p role="status" className="text-muted-foreground">{{
      checking: 'Checking DINOv3 file access…',
      verified: 'DINOv3 file access verified.',
      not_connected: 'No token is saved for this backend. Create and connect one above.',
      access_denied: 'DINOv3 access denied. Check your approval on Hugging Face and use a token with gated read access.',
    }[access]}</p>}
    {error && <p role="alert" className="text-destructive">{error}</p>}
    <div className="grid gap-2">
      <p className="text-muted-foreground">Review the model and checkpoint terms before downloading. {backend.id === 'pixal3d-cuda' ? 'The files are about 25–30 GB; allow around 40 GiB free.' : 'Allow around 25 GB free. Setup then converts the NAF checkpoint locally.'}</p>
      <div className="flex items-start gap-2">
        <Checkbox id="model-download-consent" checked={acknowledged && access === 'verified'} onCheckedChange={value => onAcknowledge(value === true)} disabled={downloadBlocked} />
        <Label htmlFor="model-download-consent" className="leading-relaxed">I reviewed the model terms and approve this download.</Label>
      </div>
    </div>
    <Button disabled={downloadBlocked || !acknowledged} onClick={onDownload}>Download model files</Button>
  </>;
}
