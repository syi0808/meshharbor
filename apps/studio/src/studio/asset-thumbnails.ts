import * as THREE from 'three';
import { configurePreviewRenderer, createPreviewScene, disposePreviewModel, frameFromObject, loadPreviewModel } from './preview-model';
import type { Asset } from './types';

const SIZE = 320;
const MAX_THUMBNAILS = 128;
const STORE_NAME = 'thumbnails';
const BACKGROUND = '#242424';

let database: Promise<IDBDatabase | null> | undefined;
let renderQueue = Promise.resolve();
let renderer: THREE.WebGLRenderer | undefined;
let webGLUnavailable = false;

function openCache() {
  database ??= new Promise<IDBDatabase | null>((resolve) => {
    const request = indexedDB.open('meshharbor.studio.thumbnails.v1', 1);
    request.onupgradeneeded = () => {
      const store = request.result.createObjectStore(STORE_NAME);
      store.createIndex('savedAt', 'savedAt');
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = request.onblocked = () => resolve(null);
  }).catch(() => null);
  return database;
}

async function readThumbnail(key: string) {
  const cache = await openCache();
  if (!cache) return null;
  return new Promise<Blob | null>((resolve) => {
    const request = cache.transaction(STORE_NAME).objectStore(STORE_NAME).get(key);
    request.onsuccess = () => resolve(request.result?.blob ?? null);
    request.onerror = () => resolve(null);
  }).catch(() => null);
}

async function saveThumbnail(key: string, blob: Blob) {
  const cache = await openCache();
  if (!cache) return;
  await new Promise<void>((resolve) => {
    const transaction = cache.transaction(STORE_NAME, 'readwrite');
    transaction.oncomplete = transaction.onabort = () => resolve();
    const store = transaction.objectStore(STORE_NAME);
    store.put({ blob, savedAt: Date.now() }, key);
    const count = store.count();
    count.onsuccess = () => {
      let excess = count.result - MAX_THUMBNAILS;
      if (excess <= 0) return;
      const oldest = store.index('savedAt').openCursor();
      oldest.onsuccess = () => {
        const cursor = oldest.result;
        if (cursor && excess > 0) {
          cursor.delete();
          excess -= 1;
          if (excess > 0) cursor.continue();
        }
      };
    };
  }).catch(() => undefined);
}

function encode(canvas: HTMLCanvasElement) {
  return new Promise<Blob>((resolve, reject) => {
    canvas.toBlob(blob => blob ? resolve(blob) : reject(new Error('Could not capture thumbnail.')), 'image/webp', 0.84);
  });
}

async function captureImage(blob: Blob, signal: AbortSignal) {
  const image = await createImageBitmap(blob);
  try {
    signal.throwIfAborted();
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = SIZE;
    const context = canvas.getContext('2d');
    if (!context) throw new Error('Image thumbnails are unavailable.');
    context.fillStyle = BACKGROUND;
    context.fillRect(0, 0, SIZE, SIZE);
    const scale = (SIZE * 0.92) / Math.max(image.width, image.height);
    const width = image.width * scale;
    const height = image.height * scale;
    context.drawImage(image, (SIZE - width) / 2, (SIZE - height) / 2, width, height);
    return await encode(canvas);
  } finally {
    image.close();
  }
}

function thumbnailRenderer() {
  if (renderer) return renderer;
  try {
    const next = new THREE.WebGLRenderer({ antialias: true, alpha: false, preserveDrawingBuffer: true });
    next.setSize(SIZE, SIZE, false);
    configurePreviewRenderer(next);
    renderer = next;
    return next;
  } catch (error) {
    webGLUnavailable = true;
    throw error;
  }
}

async function captureMesh(blob: Blob, signal: AbortSignal) {
  const model = await loadPreviewModel(blob, signal);
  try {
    signal.throwIfAborted();
    const frame = frameFromObject(model);
    if (!frame || !Number.isFinite(frame.maxDimension)) throw new Error('No renderable mesh bounds.');
    model.position.sub(new THREE.Vector3(...frame.origin));
    const scene = createPreviewScene(BACKGROUND);
    scene.add(model);
    const distance = frame.maxDimension;
    const camera = new THREE.PerspectiveCamera(36, 1, distance / 1000, distance * 100);
    camera.position.set(distance * 0.95, distance * 0.58, distance * 1.75);
    camera.lookAt(...frame.target);
    const surface = thumbnailRenderer();
    surface.render(scene, camera);
    if (surface.getContext().isContextLost()) throw new Error('Thumbnail renderer is unavailable.');
    return await encode(surface.domElement);
  } finally {
    disposePreviewModel(model);
  }
}

export async function assetThumbnail(
  asset: Asset,
  load: (asset: Asset, signal: AbortSignal) => Promise<Blob>,
  signal: AbortSignal,
) {
  const key = `${asset.kind}:${asset.sha256}`;
  const cached = await readThumbnail(key);
  if (cached) return cached;

  const capture = renderQueue.then(async () => {
    signal.throwIfAborted();
    const previous = await readThumbnail(key);
    if (previous) return previous;
    if (asset.kind === 'mesh' && webGLUnavailable) return null;
    await new Promise(resolve => setTimeout(resolve, 0));
    signal.throwIfAborted();
    const source = await load(asset, signal);
    const thumbnail = asset.kind === 'mesh' ? await captureMesh(source, signal) : await captureImage(source, signal);
    await saveThumbnail(key, thumbnail);
    return thumbnail;
  });
  renderQueue = capture.then(() => undefined, () => undefined);
  return capture;
}
