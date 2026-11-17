import { useEffect, useRef, useState } from 'react';
import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { MeshoptDecoder } from 'three/addons/libs/meshopt_decoder.module.js';
import { applyTriangleWireframe, isLocalPreviewResource, restorePreviewMaterials } from './preview-display';
import {
  previewFrameFromBounds,
  shouldPreserveComparisonCamera,
  shouldPreserveSourceTransition,
  type PreviewFrame,
} from './preview-camera';
import type { Asset } from './types';

const MAX_CANONICAL_SOURCE_FRAMES = 64;
const canonicalSourceFrameCache = new Map<string, PreviewFrame>();

export interface PreviewComparison {
  /** Stable, pair-specific identity. Keep it unchanged when switching sides. */
  groupId: string;
  /** The original asset is also the canonical source of comparison bounds. */
  sourceAsset: Asset;
  resultAssetId: string;
  side: 'source' | 'result';
}

interface Props {
  asset: Asset | null;
  loadModel: (asset: Asset) => Promise<Blob>;
  onError: (message: string) => void;
  comparison?: PreviewComparison | null;
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
      .catch((error: unknown) => {
        if (!disposed) onError(error instanceof Error ? error.message : 'Could not load the image.');
      })
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

function MeshPreview({ asset, loadModel, onError, comparison }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  const sceneRef = useRef<THREE.Scene | null>(null);
  const cameraRef = useRef<THREE.PerspectiveCamera | null>(null);
  const controlsRef = useRef<OrbitControls | null>(null);
  const currentModelRef = useRef<THREE.Group | null>(null);
  const currentAssetIdRef = useRef<string | null>(null);
  const activeComparisonGroupRef = useRef<string | null>(null);
  const onErrorRef = useRef(onError);
  const wireframePreviewRef = useRef<{
    meshes: THREE.Mesh[];
    originalMaterials: Map<THREE.Mesh, THREE.Material | THREE.Material[]>;
    material: THREE.MeshBasicMaterial;
  } | null>(null);
  const [loading, setLoading] = useState(false);
  const [modelReady, setModelReady] = useState(false);
  const [wireframe, setWireframe] = useState(false);
  const [rendererUnavailable, setRendererUnavailable] = useState(false);

  useEffect(() => { onErrorRef.current = onError; }, [onError]);

  // Keep one renderer and one set of controls alive while the viewed asset changes.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const scene = new THREE.Scene();
    scene.background = new THREE.Color('#eef0ec');
    const camera = new THREE.PerspectiveCamera(36, 1, 0.01, 1000);
    camera.position.set(3.6, 2.5, 4.2);
    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false });
    } catch {
      // WebGL is an optional preview capability; its failure must not block local asset work.
      setRendererUnavailable(true);
      return;
    }
    setRendererUnavailable(false);
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

    sceneRef.current = scene;
    cameraRef.current = camera;
    controlsRef.current = controls;

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

    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      controls.dispose();
      renderer.dispose();
      const preview = wireframePreviewRef.current;
      if (preview) {
        restorePreviewMaterials(preview.originalMaterials);
        preview.material.dispose();
        wireframePreviewRef.current = null;
      }
      if (currentModelRef.current) {
        scene.remove(currentModelRef.current);
        disposeGroup(currentModelRef.current);
        currentModelRef.current = null;
      }
      sceneRef.current = null;
      cameraRef.current = null;
      controlsRef.current = null;
      currentAssetIdRef.current = null;
      activeComparisonGroupRef.current = null;
      host.replaceChildren();
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    const scene = sceneRef.current;
    const camera = cameraRef.current;
    const controls = controlsRef.current;
    const preview = wireframePreviewRef.current;
    if (preview) {
      restorePreviewMaterials(preview.originalMaterials);
      preview.material.dispose();
      wireframePreviewRef.current = null;
    }
    if (currentModelRef.current && scene) {
      scene.remove(currentModelRef.current);
      disposeGroup(currentModelRef.current);
      currentModelRef.current = null;
    }
    setModelReady(false);
    setLoading(false);

    if (!asset || asset.kind !== 'mesh' || !scene || !camera || !controls) {
      currentAssetIdRef.current = null;
      activeComparisonGroupRef.current = null;
      return () => { disposed = true; };
    }

    const validComparison = comparisonForAsset(asset, comparison);
    const previousAssetId = currentAssetIdRef.current;
    const previousGroupId = activeComparisonGroupRef.current;
    const preserveCamera = validComparison !== null && (
      shouldPreserveComparisonCamera(previousGroupId, validComparison.groupId)
      || shouldPreserveSourceTransition(previousAssetId, validComparison.sourceAsset.id)
    );
    if (!preserveCamera) {
      activeComparisonGroupRef.current = null;
      setWireframe(false);
    }
    setLoading(true);

    const loadScene = async (source: Asset): Promise<THREE.Group> => {
      const blob = await loadModel(source);
      if (disposed) throw new PreviewRequestCancelled();
      const buffer = await blob.arrayBuffer();
      if (disposed) throw new PreviewRequestCancelled();
      ensureEmbeddedGlbResources(buffer);
      const manager = new THREE.LoadingManager();
      manager.setURLModifier((url) => {
        if (isLocalPreviewResource(url, window.location.origin)) return url;
        throw new Error('External GLB resources are blocked in local preview. Embed textures and buffers in the GLB first.');
      });
      const loader = new GLTFLoader(manager);
      loader.setMeshoptDecoder(MeshoptDecoder);
      const gltf = await loader.parseAsync(buffer, '');
      if (disposed) {
        disposeGroup(gltf.scene);
        throw new PreviewRequestCancelled();
      }
      return gltf.scene;
    };

    void (async () => {
      let model: THREE.Group | null = null;
      let sourceFrame: PreviewFrame | null = null;
      try {
        const sourceFrameKey = validComparison ? canonicalSourceFrameKey(validComparison.sourceAsset) : null;
        if (validComparison && !sourceFrame && sourceFrameKey) {
          sourceFrame = canonicalSourceFrameCache.get(sourceFrameKey) ?? null;
        }

        if (validComparison && !sourceFrame && asset.id !== validComparison.sourceAsset.id) {
          const sourceModel = await loadScene(validComparison.sourceAsset);
          sourceFrame = frameFromObject(sourceModel);
          disposeGroup(sourceModel);
          if (!sourceFrame) throw new Error('The original GLB has no finite, non-degenerate preview bounds.');
        }
        if (disposed) return;

        model = await loadScene(asset);
        if (disposed) {
          disposeGroup(model);
          return;
        }

        const modelFrame = frameFromObject(model);
        const frame = sourceFrame ?? modelFrame;
        if (!frame) throw new Error('The GLB has no finite, non-degenerate preview bounds.');
        if (validComparison && sourceFrameKey) cacheCanonicalSourceFrame(sourceFrameKey, frame);
        const origin = validComparison ? frame.origin : modelFrame?.origin ?? frame.origin;
        model.position.sub(new THREE.Vector3(...origin));

        const meshes: THREE.Mesh[] = [];
        model.traverse((node) => {
          if (node instanceof THREE.Mesh) meshes.push(node);
        });
        const nextPreview = {
          meshes,
          originalMaterials: new Map<THREE.Mesh, THREE.Material | THREE.Material[]>(),
          material: new THREE.MeshBasicMaterial({ color: '#263a32', wireframe: true, toneMapped: false }),
        };
        wireframePreviewRef.current = nextPreview;
        currentModelRef.current = model;
        currentAssetIdRef.current = asset.id;
        scene.add(model);

        if (!preserveCamera) fitCameraToFrame(camera, controls, frame);
        activeComparisonGroupRef.current = validComparison?.groupId ?? null;
        setModelReady(meshes.length > 0);
      } catch (error: unknown) {
        if (model && currentModelRef.current !== model) disposeGroup(model);
        if (!disposed && !(error instanceof PreviewRequestCancelled)) {
          onErrorRef.current(error instanceof Error ? error.message : 'Could not load the asset.');
        }
      } finally {
        if (!disposed) setLoading(false);
      }
    })();

    return () => { disposed = true; };
  }, [
    asset?.id,
    asset?.kind,
    loadModel,
    comparison?.groupId,
    comparison?.sourceAsset.id,
    comparison?.sourceAsset.sha256,
    comparison?.resultAssetId,
    comparison?.side,
  ]);

  useEffect(() => {
    const preview = wireframePreviewRef.current;
    if (!preview) return;
    if (wireframe) {
      applyTriangleWireframe(preview.meshes, preview.material, preview.originalMaterials);
    } else {
      restorePreviewMaterials(preview.originalMaterials);
    }
  }, [wireframe, modelReady]);

  return (
    <div className="preview-stage">
      <div className="preview-canvas" ref={hostRef} aria-label="Interactive 3D model preview" />
      {modelReady && (
        <div className="preview-display-toggle" role="group" aria-label="3D preview display">
          <button type="button" aria-pressed={!wireframe} onClick={() => setWireframe(false)}>Material view</button>
          <button type="button" aria-pressed={wireframe} onClick={() => setWireframe(true)}>Triangle wireframe</button>
        </div>
      )}
      {rendererUnavailable && asset && (
        <div className="preview-empty preview-fallback" role="status">
          <p>3D preview unavailable</p>
          <span>WebGL could not start in this browser. Local optimize and export are still available.</span>
        </div>
      )}
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
      {asset && !loading && !rendererUnavailable && <div className="preview-hint"><span className="mouse-glyph">↻</span> Drag to orbit <i /> Scroll to zoom</div>}
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

function comparisonForAsset(asset: Asset, comparison: PreviewComparison | null | undefined): PreviewComparison | null {
  if (!comparison
    || !comparison.groupId.trim()
    || asset.kind !== 'mesh'
    || comparison.sourceAsset.kind !== 'mesh'
    || comparison.sourceAsset.id === comparison.resultAssetId
    || (comparison.side !== 'source' && comparison.side !== 'result')) {
    return null;
  }
  if (comparison.side === 'result' && asset.source_asset_id !== comparison.sourceAsset.id) return null;
  const expectedAssetId = comparison.side === 'source'
    ? comparison.sourceAsset.id
    : comparison.resultAssetId;
  return asset.id === expectedAssetId ? comparison : null;
}

function canonicalSourceFrameKey(asset: Asset): string {
  return `${asset.id}:${asset.sha256}`;
}

function cacheCanonicalSourceFrame(key: string, frame: PreviewFrame) {
  canonicalSourceFrameCache.delete(key);
  canonicalSourceFrameCache.set(key, {
    origin: [...frame.origin],
    target: [...frame.target],
    size: [...frame.size],
    maxDimension: frame.maxDimension,
  });
  if (canonicalSourceFrameCache.size > MAX_CANONICAL_SOURCE_FRAMES) {
    const oldestKey = canonicalSourceFrameCache.keys().next().value;
    if (oldestKey !== undefined) canonicalSourceFrameCache.delete(oldestKey);
  }
}

function frameFromObject(model: THREE.Object3D): PreviewFrame | null {
  const bounds = new THREE.Box3().setFromObject(model);
  if (bounds.isEmpty()) return null;
  const min = bounds.min;
  const max = bounds.max;
  return previewFrameFromBounds({
    min: [min.x, min.y, min.z],
    max: [max.x, max.y, max.z],
  });
}

function fitCameraToFrame(camera: THREE.PerspectiveCamera, controls: OrbitControls, frame: PreviewFrame) {
  const target = new THREE.Vector3(...frame.target);
  const distance = frame.maxDimension * 1.65;
  camera.position.set(target.x + distance, target.y + distance * 0.72, target.z + distance * 1.15);
  camera.near = frame.maxDimension / 1000;
  camera.far = frame.maxDimension * 100;
  camera.updateProjectionMatrix();
  controls.target.copy(target);
  controls.minDistance = frame.maxDimension * 0.2;
  controls.maxDistance = frame.maxDimension * 12;
  controls.update();
}

class PreviewRequestCancelled extends Error {
  constructor() {
    super('Preview request was superseded.');
    this.name = 'PreviewRequestCancelled';
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

