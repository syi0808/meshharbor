import { Button } from '@/components/ui/button';
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from '@/components/ui/empty';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { Spinner } from '@/components/ui/spinner';
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group';
import { CornersOutIcon } from '@phosphor-icons/react';
import { useEffect, useRef, useState } from 'react';
import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { applyPreviewMaterial, applyPreviewPartMaterials, restorePreviewMaterials, separatedPartInfo } from './preview-display';
import { configurePreviewRenderer, createPreviewScene, disposePreviewModel, frameFromObject, loadPreviewModel, previewPrimitiveIndex } from './preview-model';
import {
  shouldPreserveComparisonCamera,
  shouldPreserveSourceTransition,
  type PreviewFrame,
} from './preview-camera';
import type { Asset } from './types';
import type { StudioTheme } from './theme';

const MAX_CANONICAL_SOURCE_FRAMES = 64;
const canonicalSourceFrameCache = new Map<string, PreviewFrame>();
const viewportLayout = 'relative isolate min-h-0 min-w-0 flex-1 overflow-hidden bg-muted dark:bg-neutral-800';
type PreviewMode = 'material' | 'solid' | 'wireframe' | 'parts';

interface PreviewDisplay {
  meshes: THREE.Mesh[];
  originalMaterials: Map<THREE.Mesh, THREE.Material | THREE.Material[]>;
  wireframe: THREE.MeshBasicMaterial;
  solid: THREE.MeshStandardMaterial;
  parts?: {
    meshParts: Map<THREE.Mesh, number>;
    palette: THREE.MeshStandardMaterial[];
    dimmed: THREE.MeshStandardMaterial;
  };
}

function disposeDisplay(preview: PreviewDisplay) {
  restorePreviewMaterials(preview.originalMaterials);
  preview.wireframe.dispose();
  preview.solid.dispose();
  preview.parts?.dimmed.dispose();
  preview.parts?.palette.forEach(material => material.dispose());
}

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
  loadModel: (asset: Asset, signal?: AbortSignal) => Promise<Blob>;
  onError: (message: string) => void;
  comparison?: PreviewComparison | null;
  theme: StudioTheme;
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
    const controller = new AbortController();
    let objectUrl: string | null = null;
    setLoading(true);
    void loadModel(asset, controller.signal)
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
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [asset.id, loadModel, onError]);

  return (
    <div className={viewportLayout}>
      <div className="absolute inset-0 grid place-items-center overflow-hidden p-6">
        {imageUrl ? <img src={imageUrl} alt={`Preview of ${asset.name}`} className="max-h-full max-w-full object-contain" /> : <p className="text-sm text-muted-foreground">Local source image</p>}
      </div>
      {loading && <PreviewLoading>Loading image…</PreviewLoading>}
      <p className="absolute right-4 bottom-4 text-xs text-muted-foreground">Source image · stays on this device</p>
    </div>
  );
}

function PreviewLoading({ children }: { children: string }) {
  return <div className="absolute inset-0 grid place-items-center" role="status">
    <span className="inline-flex items-center gap-2 rounded-md border bg-popover px-3 py-2 text-xs text-popover-foreground">
      <Spinner aria-hidden="true" /> {children}
    </span>
  </div>;
}

function MeshPreview({ asset, loadModel, onError, comparison, theme }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  const sceneRef = useRef<THREE.Scene | null>(null);
  const cameraRef = useRef<THREE.PerspectiveCamera | null>(null);
  const controlsRef = useRef<OrbitControls | null>(null);
  const currentModelRef = useRef<THREE.Group | null>(null);
  const frameRef = useRef<PreviewFrame | null>(null);
  const currentAssetIdRef = useRef<string | null>(null);
  const activeComparisonGroupRef = useRef<string | null>(null);
  const onErrorRef = useRef(onError);
  const displayRef = useRef<PreviewDisplay | null>(null);
  const [loading, setLoading] = useState(false);
  const [modelReady, setModelReady] = useState(false);
  const [display, setDisplay] = useState<PreviewMode>('material');
  const [parts, setParts] = useState<{ assetId: string; count: number; grouped: boolean } | null>(null);
  const [partSelection, setPartSelection] = useState<{ assetId: string; index: number } | null>(null);
  const visibleParts = parts && parts.assetId === asset?.id ? parts : null;
  const focusedPart = partSelection && partSelection.assetId === asset?.id ? partSelection.index : null;
  const visibleDisplay = display === 'parts' && !visibleParts ? 'material' : display;
  const [rendererUnavailable, setRendererUnavailable] = useState(false);

  useEffect(() => { onErrorRef.current = onError; }, [onError]);

  // Keep one renderer and one set of controls alive while the viewed asset changes.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const scene = createPreviewScene();
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
    configurePreviewRenderer(renderer);
    host.replaceChildren(renderer.domElement);

    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.dampingFactor = 0.07;
    controls.minDistance = 0.1;
    controls.maxDistance = 300;

    sceneRef.current = scene;
    cameraRef.current = camera;
    controlsRef.current = controls;

    let visible = false;
    const resize = () => {
      const width = host.clientWidth;
      const height = host.clientHeight;
      visible = width > 0 && height > 0;
      if (!visible) return;
      renderer.setSize(width, height, false);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
    };
    const observer = new ResizeObserver(resize);
    observer.observe(host);
    resize();

    let frame = 0;
    const render = () => {
      if (visible) {
        controls.update();
        renderer.render(scene, camera);
      }
      frame = requestAnimationFrame(render);
    };
    render();

    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      controls.dispose();
      renderer.dispose();
      const preview = displayRef.current;
      if (preview) {
        disposeDisplay(preview);
        displayRef.current = null;
      }
      if (currentModelRef.current) {
        scene.remove(currentModelRef.current);
        disposePreviewModel(currentModelRef.current);
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
    if (sceneRef.current) sceneRef.current.background = new THREE.Color(theme === 'dark' ? '#202020' : '#eeeeef');
  }, [theme]);

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    const scene = sceneRef.current;
    const camera = cameraRef.current;
    const controls = controlsRef.current;
    const preview = displayRef.current;
    if (preview) {
      disposeDisplay(preview);
      displayRef.current = null;
    }
    frameRef.current = null;
    if (currentModelRef.current && scene) {
      scene.remove(currentModelRef.current);
      disposePreviewModel(currentModelRef.current);
      currentModelRef.current = null;
    }
    setModelReady(false);
    setParts(null);
    setLoading(false);

    if (!asset || asset.kind !== 'mesh' || !scene || !camera || !controls) {
      currentAssetIdRef.current = null;
      activeComparisonGroupRef.current = null;
      return () => controller.abort();
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
      setDisplay('material');
    }
    setLoading(true);

    const loadScene = async (source: Asset) => loadPreviewModel(await loadModel(source, signal), signal);

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
          disposePreviewModel(sourceModel);
          if (!sourceFrame) throw new Error('The original GLB has no finite, non-degenerate preview bounds.');
        }
        if (signal.aborted) return;

        model = await loadScene(asset);
        if (signal.aborted) {
          disposePreviewModel(model);
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
        const nextPreview: PreviewDisplay = {
          meshes,
          originalMaterials: new Map<THREE.Mesh, THREE.Material | THREE.Material[]>(),
          wireframe: new THREE.MeshBasicMaterial({ color: '#a7c7ba', wireframe: true, toneMapped: false }),
          solid: new THREE.MeshStandardMaterial({ color: '#a6a6a6', roughness: 0.82 }),
        };
        const partInfo = separatedPartInfo(asset.metadata);
        if (partInfo) {
          const meshParts = new Map<THREE.Mesh, number>();
          for (const mesh of meshes) {
            const index = previewPrimitiveIndex(mesh);
            if (index !== undefined && index >= 0 && index < partInfo.count) meshParts.set(mesh, index);
          }
          if (meshParts.size === meshes.length && new Set(meshParts.values()).size === partInfo.count) {
            nextPreview.parts = {
              meshParts,
              palette: Array.from({ length: partInfo.count }, (_, index) => new THREE.MeshStandardMaterial({
                color: new THREE.Color().setHSL((0.035 + index * 0.61803398875) % 1, 0.72, 0.49, THREE.SRGBColorSpace),
                roughness: 0.8,
                side: THREE.DoubleSide,
              })),
              dimmed: new THREE.MeshStandardMaterial({ color: '#494d53', roughness: 0.85, side: THREE.DoubleSide }),
            };
            setParts({ assetId: asset.id, ...partInfo });
          }
        }
        displayRef.current = nextPreview;
        frameRef.current = frame;
        currentModelRef.current = model;
        currentAssetIdRef.current = asset.id;
        scene.add(model);

        if (!preserveCamera) fitCameraToFrame(camera, controls, frame);
        activeComparisonGroupRef.current = validComparison?.groupId ?? null;
        setModelReady(meshes.length > 0);
      } catch (error: unknown) {
        if (model && currentModelRef.current !== model) disposePreviewModel(model);
        if (!signal.aborted) {
          onErrorRef.current(error instanceof Error ? error.message : 'Could not load the asset.');
        }
      } finally {
        if (!signal.aborted) setLoading(false);
      }
    })();

    return () => controller.abort();
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
    const preview = displayRef.current;
    if (!preview) return;
    restorePreviewMaterials(preview.originalMaterials);
    if (visibleDisplay === 'parts' && preview.parts) {
      applyPreviewPartMaterials(preview.parts.meshParts, preview.parts.palette, preview.parts.dimmed, focusedPart, preview.originalMaterials);
    } else if (visibleDisplay === 'solid' || visibleDisplay === 'wireframe') {
      applyPreviewMaterial(preview.meshes, preview[visibleDisplay], preview.originalMaterials);
    }
  }, [asset?.id, visibleDisplay, focusedPart, modelReady]);

  function fitView() {
    if (cameraRef.current && controlsRef.current && frameRef.current) {
      fitCameraToFrame(cameraRef.current, controlsRef.current, frameRef.current);
    }
  }

  return (
    <div className={viewportLayout}>
      <div className="absolute inset-0 [&>canvas]:block [&>canvas]:size-full" ref={hostRef} aria-label="Interactive 3D model preview" />
      {modelReady && (
        <div className="absolute bottom-2 left-2 flex max-w-[calc(100%-1rem)] flex-wrap items-center gap-1 rounded-lg border bg-popover p-1 sm:bottom-4 sm:left-4" role="group" aria-label="Viewport controls">
          <ToggleGroup type="single" orientation="horizontal" size="sm" spacing={1} value={visibleDisplay} aria-label="Display mode"
            onValueChange={value => {
              if (value === 'material' || value === 'solid' || value === 'wireframe' || value === 'parts') setDisplay(value);
            }}>
            <ToggleGroupItem value="material" aria-label="Material view">Material</ToggleGroupItem>
            <ToggleGroupItem value="solid" aria-label="Solid view">Solid</ToggleGroupItem>
            <ToggleGroupItem value="wireframe" aria-label="Triangle wireframe">Wireframe</ToggleGroupItem>
            {visibleParts && <ToggleGroupItem value="parts" aria-label="Part highlighting">Parts</ToggleGroupItem>}
          </ToggleGroup>
          <Button type="button" variant="ghost" size="icon-sm" aria-label="Fit view" title="Fit view" onClick={fitView}><CornersOutIcon aria-hidden="true" /></Button>
          {visibleDisplay === 'parts' && visibleParts && <NativeSelect aria-label="Highlight part" size="sm" value={focusedPart ?? 'all'}
            onChange={event => setPartSelection(event.target.value === 'all' ? null : { assetId: visibleParts.assetId, index: Number(event.target.value) })}>
            <NativeSelectOption value="all">All parts</NativeSelectOption>
            {Array.from({ length: visibleParts.count }, (_, index) =>
              <NativeSelectOption key={index} value={index}>
                {visibleParts.grouped && index === visibleParts.count - 1 ? 'Grouped fragments' : `Part ${index + 1}`}
              </NativeSelectOption>)}
          </NativeSelect>}
        </div>
      )}
      {rendererUnavailable && asset && (
        <Empty className="absolute inset-0" role="status">
          <EmptyHeader>
            <EmptyTitle>3D preview unavailable</EmptyTitle>
            <EmptyDescription>WebGL could not start in this browser. Local optimize and export are still available.</EmptyDescription>
          </EmptyHeader>
        </Empty>
      )}
      {loading && <PreviewLoading>Loading preview…</PreviewLoading>}
      {modelReady && <p className="absolute right-4 bottom-4 hidden text-xs text-muted-foreground lg:block">Drag to orbit · Scroll to zoom</p>}
    </div>
  );
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

function fitCameraToFrame(camera: THREE.PerspectiveCamera, controls: OrbitControls, frame: PreviewFrame) {
  const target = new THREE.Vector3(...frame.target);
  const distance = frame.maxDimension * 1.65 / Math.min(camera.aspect, 1);
  camera.position.set(target.x + distance, target.y + distance * 0.72, target.z + distance * 1.15);
  camera.near = frame.maxDimension / 1000;
  camera.far = frame.maxDimension * 100;
  camera.updateProjectionMatrix();
  controls.target.copy(target);
  controls.minDistance = frame.maxDimension * 0.2;
  controls.maxDistance = frame.maxDimension * 12;
  controls.update();
}

