import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { MeshoptDecoder } from 'three/addons/libs/meshopt_decoder.module.js';
import { previewFrameFromBounds } from './preview-camera';
import { isLocalPreviewResource } from './preview-display';

export async function loadPreviewModel(blob: Blob, signal: AbortSignal) {
  signal.throwIfAborted();
  const buffer = await blob.arrayBuffer();
  signal.throwIfAborted();
  ensureEmbeddedGlbResources(buffer);
  const manager = new THREE.LoadingManager();
  manager.setURLModifier((url) => {
    if (isLocalPreviewResource(url, window.location.origin)) return url;
    throw new Error('External GLB resources are blocked in local preview. Embed textures and buffers in the GLB first.');
  });
  const loader = new GLTFLoader(manager);
  loader.setMeshoptDecoder(MeshoptDecoder);
  const { scene } = await loader.parseAsync(buffer, '');
  if (signal.aborted) {
    disposePreviewModel(scene);
    signal.throwIfAborted();
  }
  return scene;
}

function ensureEmbeddedGlbResources(buffer: ArrayBuffer) {
  const view = new DataView(buffer);
  if (view.byteLength < 20 || view.getUint32(0, true) !== 0x46546c67 || view.getUint32(4, true) !== 2) {
    throw new Error('Preview requires a valid GLB version 2 file.');
  }
  const jsonLength = view.getUint32(12, true);
  if (view.getUint32(16, true) !== 0x4e4f534a || 20 + jsonLength > view.byteLength) {
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

export function createPreviewScene(background = '#202020') {
  const scene = new THREE.Scene();
  scene.background = new THREE.Color(background);
  scene.add(new THREE.HemisphereLight(0xffffff, 0x8a928b, 2.1));
  const key = new THREE.DirectionalLight(0xffffff, 2.4);
  key.position.set(4, 6, 5);
  scene.add(key);
  const fill = new THREE.DirectionalLight(0xc9ddff, 1.2);
  fill.position.set(-4, 2, -3);
  scene.add(fill);
  return scene;
}

export function configurePreviewRenderer(renderer: THREE.WebGLRenderer) {
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.1;
}

export function frameFromObject(model: THREE.Object3D) {
  const bounds = new THREE.Box3().setFromObject(model);
  if (bounds.isEmpty()) return null;
  const { min, max } = bounds;
  return previewFrameFromBounds({ min: [min.x, min.y, min.z], max: [max.x, max.y, max.z] });
}

export function disposePreviewModel(group: THREE.Group) {
  const geometries = new Set<THREE.BufferGeometry>();
  const materials = new Set<THREE.Material>();
  const textures = new Set<THREE.Texture>();
  const bitmaps = new Set<ImageBitmap>();
  group.traverse(node => {
    if (!(node instanceof THREE.Mesh || node instanceof THREE.Line || node instanceof THREE.Points)) return;
    geometries.add(node.geometry);
    for (const material of Array.isArray(node.material) ? node.material : [node.material]) {
      materials.add(material);
    }
  });
  for (const geometry of geometries) geometry.dispose();
  for (const material of materials) {
    for (const value of Object.values(material)) {
      if (value instanceof THREE.Texture) textures.add(value);
    }
    material.dispose();
  }
  for (const texture of textures) {
    if (typeof ImageBitmap !== 'undefined' && texture.image instanceof ImageBitmap) bitmaps.add(texture.image);
    texture.dispose();
  }
  for (const bitmap of bitmaps) bitmap.close();
}
