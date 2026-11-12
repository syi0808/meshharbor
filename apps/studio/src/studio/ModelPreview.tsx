import { useEffect, useRef, useState } from 'react';
import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import type { Asset } from './types';

interface Props {
  asset: Asset | null;
  loadModel: (asset: Asset) => Promise<Blob>;
  onError: (message: string) => void;
}

export function ModelPreview(props: Props) {
  if (props.asset?.kind === 'image') return <ImagePreview {...props} asset={props.asset} />;
  return <MeshPreview {...props} />;
}

function ImagePreview({ asset, loadModel, onError }: Props & { asset: Asset }) {
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    let disposed = false;
    let objectUrl: string | null = null;
    setLoading(true);
    void loadModel(asset)
      .then((blob) => {
        objectUrl = URL.createObjectURL(blob);
        if (disposed) URL.revokeObjectURL(objectUrl);
        else setImageUrl(objectUrl);
      })
      .catch((error: unknown) => onError(error instanceof Error ? error.message : 'Could not load the image.'))
      .finally(() => { if (!disposed) setLoading(false); });
    return () => {
      disposed = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [asset.id, loadModel, onError]);

  return (
    <div className="preview-stage image-preview-stage">
      <div className="image-preview-body">
        {imageUrl ? <img src={imageUrl} alt={`Preview of ${asset.name}`} /> : <div className="image-preview-empty">Local source image</div>}
      </div>
      {loading && <div className="preview-loading"><span className="spinner" /> Loading image…</div>}
      <div className="preview-hint">Source image · stays on this device</div>
    </div>
  );
}

function MeshPreview({ asset, loadModel, onError }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    const host = hostRef.current;
    if (!host || !asset || asset.kind !== 'mesh') return;

    let disposed = false;
    let model: THREE.Group | null = null;
    const scene = new THREE.Scene();
    scene.background = new THREE.Color('#eef0ec');
    const camera = new THREE.PerspectiveCamera(36, 1, 0.01, 1000);
    camera.position.set(3.6, 2.5, 4.2);
    const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.toneMappingExposure = 1.1;
    host.replaceChildren(renderer.domElement);

    scene.add(new THREE.HemisphereLight(0xffffff, 0x8a928b, 2.1));
    const key = new THREE.DirectionalLight(0xffffff, 2.4);
    key.position.set(4, 6, 5);
    scene.add(key);
    const fill = new THREE.DirectionalLight(0xc9ddff, 1.2);
    fill.position.set(-4, 2, -3);
    scene.add(fill);
    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.dampingFactor = 0.07;
    controls.minDistance = 0.1;
    controls.maxDistance = 300;

    const resize = () => {
      const width = Math.max(host.clientWidth, 1);
      const height = Math.max(host.clientHeight, 1);
      renderer.setSize(width, height, false);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
    };
    const observer = new ResizeObserver(resize);
    observer.observe(host);
    resize();

    let frame = 0;
    const render = () => {
      controls.update();
      renderer.render(scene, camera);
      frame = requestAnimationFrame(render);
    };
    render();

    setLoading(true);
    void loadModel(asset)
      .then(async (blob) => {
        if (disposed) return;
        const buffer = await blob.arrayBuffer();
        ensureEmbeddedGlbResources(buffer);
        const manager = new THREE.LoadingManager();
        manager.setURLModifier((url) => {
          if (url.startsWith('data:')) return url;
          throw new Error('External GLB resources are blocked in local preview. Embed textures and buffers in the GLB first.');
        });
        const gltf = await new GLTFLoader(manager).parseAsync(buffer, '');
        if (disposed) {
          disposeGroup(gltf.scene);
          return;
        }
        model = gltf.scene;
        const bounds = new THREE.Box3().setFromObject(model);
        const size = bounds.getSize(new THREE.Vector3());
        const center = bounds.getCenter(new THREE.Vector3());
        model.position.sub(center);
        const maxDim = Math.max(size.x, size.y, size.z, 0.001);
        const distance = maxDim * 1.65;
        camera.position.set(distance, distance * 0.72, distance * 1.15);
        camera.near = maxDim / 1000;
        camera.far = maxDim * 100;
        camera.updateProjectionMatrix();
        controls.target.set(0, 0, 0);
        controls.minDistance = maxDim * 0.2;
        controls.maxDistance = maxDim * 12;
        scene.add(model);
      })
      .catch((error: unknown) => onError(error instanceof Error ? error.message : 'Could not load the asset.'))
      .finally(() => {
        if (!disposed) setLoading(false);
      });

    return () => {
      disposed = true;
      cancelAnimationFrame(frame);
      observer.disconnect();
      controls.dispose();
      if (model) disposeGroup(model);
      renderer.dispose();
      host.replaceChildren();
    };
  }, [asset?.id, loadModel, onError]);

  return (
    <div className="preview-stage">
      <div className="preview-canvas" ref={hostRef} aria-label="Interactive 3D model preview" />
      {!asset && (
        <div className="preview-empty">
          <div className="empty-orbit orbit-one" />
          <div className="empty-orbit orbit-two" />
          <div className="empty-cube"><span /><span /><span /><span /></div>
          <p>Import a GLB or source image to get started</p>
          <span>Your model stays on this device</span>
        </div>
      )}
      {loading && <div className="preview-loading"><span className="spinner" /> Loading preview…</div>}
      {asset && !loading && <div className="preview-hint"><span className="mouse-glyph">↻</span> Drag to orbit <i /> Scroll to zoom</div>}
    </div>
  );
}

function ensureEmbeddedGlbResources(buffer: ArrayBuffer) {
  const view = new DataView(buffer);
  if (view.byteLength < 20 || view.getUint32(0, true) !== 0x46546c67 || view.getUint32(4, true) !== 2) {
    throw new Error('Preview requires a valid GLB version 2 file.');
  }
  const jsonLength = view.getUint32(12, true);
  const jsonType = view.getUint32(16, true);
  if (jsonType !== 0x4e4f534a || 20 + jsonLength > view.byteLength) {
    throw new Error('The GLB JSON chunk is invalid.');
  }
  const jsonBytes = new Uint8Array(buffer, 20, jsonLength);
  let document: { buffers?: Array<{ uri?: string }>; images?: Array<{ uri?: string }> };
  try {
    document = JSON.parse(new TextDecoder().decode(jsonBytes)) as typeof document;
  } catch {
    throw new Error('The GLB JSON chunk could not be read.');
  }
  const externalResource = [...(document.buffers ?? []), ...(document.images ?? [])]
    .some((resource) => resource.uri !== undefined && !resource.uri.startsWith('data:'));
  if (externalResource) {
    throw new Error('This GLB references external buffers or textures. The local preview blocks external requests; embed those resources first.');
  }
}

function disposeGroup(group: THREE.Group) {
  group.traverse((node) => {
    if (!(node instanceof THREE.Mesh)) return;
    node.geometry.dispose();
    const materials = Array.isArray(node.material) ? node.material : [node.material];
    for (const material of materials) {
      for (const value of Object.values(material)) {
        if (value instanceof THREE.Texture) value.dispose();
      }
      material.dispose();
    }
  });
}
