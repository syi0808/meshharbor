import assert from 'node:assert/strict';
import { PNG } from 'pngjs';

const segments = 12;
export const fixtureTriangles = 4 * segments ** 2;

function encodePng(upper, lower) {
  const png = new PNG({ width: 8, height: 8 });
  for (let offset = 0; offset < png.data.length; offset += 4) {
    png.data.set(offset < png.data.length / 2 ? upper : lower, offset);
  }
  return PNG.sync.write(png);
}

function panel(startX, slope, material, addAccessor) {
  const positions = [];
  const normals = [];
  const uv = [];
  const indices = [];
  const normalLength = Math.hypot(slope, 1);
  const width = segments + 1;
  for (let row = 0; row < width; row += 1) {
    for (let column = 0; column < width; column += 1) {
      const horizontal = column / segments;
      const vertical = row / segments;
      const positionX = startX + horizontal;
      positions.push(positionX, -0.75 + 1.5 * vertical, slope * positionX);
      normals.push(-slope / normalLength, 0, 1 / normalLength);
      uv.push(horizontal, vertical);
      if (column < segments && row < segments) {
        const bottomLeft = row * width + column;
        const topLeft = bottomLeft + width;
        indices.push(bottomLeft, bottomLeft + 1, topLeft + 1, bottomLeft, topLeft + 1, topLeft);
      }
    }
  }
  return {
    attributes: {
      POSITION: addAccessor(positions, 'VEC3', {
        min: [startX, -0.75, 0],
        max: [startX + 1, 0.75, Math.abs(slope)],
      }),
      NORMAL: addAccessor(normals, 'VEC3'),
      TEXCOORD_0: addAccessor(uv, 'VEC2'),
    },
    indices: addAccessor(indices, 'SCALAR'),
    material,
  };
}

function chunk(type, bytes, padding) {
  const aligned = Buffer.alloc(Math.ceil(bytes.length / 4) * 4, padding);
  bytes.copy(aligned);
  const header = Buffer.alloc(8);
  header.writeUInt32LE(aligned.length);
  header.write(type, 4);
  return Buffer.concat([header, aligned]);
}

export function texturedGlb() {
  const payload = [];
  const bufferViews = [];
  const accessors = [];
  let byteOffset = 0;
  function addView(bytes, target) {
    const index = bufferViews.push({ buffer: 0, byteOffset, byteLength: bytes.length, target }) - 1;
    const padded = Buffer.alloc(Math.ceil(bytes.length / 4) * 4);
    bytes.copy(padded);
    payload.push(padded);
    byteOffset += padded.length;
    return index;
  }
  function addAccessor(values, type, bounds) {
    const isIndex = type === 'SCALAR';
    const componentSize = isIndex ? 2 : 4;
    const componentCount = type === 'VEC3' ? 3 : type === 'VEC2' ? 2 : 1;
    const bytes = Buffer.alloc(values.length * componentSize);
    values.forEach((value, index) => {
      if (isIndex) bytes.writeUInt16LE(value, index * componentSize);
      else bytes.writeFloatLE(value, index * componentSize);
    });
    return accessors.push({
      bufferView: addView(bytes, isIndex ? 34963 : 34962),
      componentType: isIndex ? 5123 : 5126,
      count: values.length / componentCount,
      type,
      ...bounds,
    }) - 1;
  }

  const primitives = [panel(-1, -0.125, 0, addAccessor), panel(0, 0.125, 1, addAccessor)];
  const images = [
    encodePng([225, 25, 16, 255], [16, 190, 25, 255]),
    encodePng([25, 47, 230, 255], [230, 194, 19, 255]),
    encodePng([255, 90, 24, 255], [255, 210, 115, 255]),
  ].map((png) => ({ mimeType: 'image/png', bufferView: addView(png) }));
  const binary = Buffer.concat(payload);
  const document = {
    asset: { version: '2.0' },
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [{ mesh: 0 }],
    meshes: [{ primitives }],
    materials: [
      { name: 'Painted panel', pbrMetallicRoughness: {
        baseColorTexture: { index: 0 }, metallicRoughnessTexture: { index: 2 },
        metallicFactor: 0.3, roughnessFactor: 0.85,
      } },
      { name: 'Coated panel', pbrMetallicRoughness: {
        baseColorTexture: { index: 1 }, metallicRoughnessTexture: { index: 2 },
        metallicFactor: 0.7, roughnessFactor: 0.55,
      } },
    ],
    textures: images.map((_, source) => ({ source, sampler: 0 })),
    samplers: [{ magFilter: 9728, minFilter: 9728, wrapS: 33071, wrapT: 33071 }],
    images,
    buffers: [{ byteLength: binary.length }],
    bufferViews,
    accessors,
  };

  const jsonChunk = chunk('JSON', Buffer.from(JSON.stringify(document)), 0x20);
  const binChunk = chunk('BIN\0', binary, 0);
  const header = Buffer.alloc(12);
  header.write('glTF');
  header.writeUInt32LE(2, 4);
  header.writeUInt32LE(header.length + jsonChunk.length + binChunk.length, 8);
  return Buffer.concat([header, jsonChunk, binChunk]);
}

export function inspectTexturedGlb(bytes) {
  assert.equal(bytes.toString('ascii', 0, 4), 'glTF');
  assert.equal(bytes.readUInt32LE(4), 2);
  assert.equal(bytes.readUInt32LE(8), bytes.length);
  const jsonEnd = 20 + bytes.readUInt32LE(12);
  assert.equal(bytes.toString('ascii', 16, 20), 'JSON');
  assert.equal(bytes.toString('ascii', jsonEnd + 4, jsonEnd + 8), 'BIN\0');
  assert.equal(jsonEnd + 8 + bytes.readUInt32LE(jsonEnd), bytes.length);
  const document = JSON.parse(bytes.toString('utf8', 20, jsonEnd));
  const binary = bytes.subarray(jsonEnd + 8);
  const primitives = document.meshes.flatMap(({ primitives }) => primitives);

  function viewBytes(index) {
    const view = document.bufferViews[index];
    const start = view.byteOffset ?? 0;
    assert.equal(view.buffer, 0);
    assert.ok(start >= 0 && view.byteLength > 0 && start + view.byteLength <= binary.length);
    return binary.subarray(start, start + view.byteLength);
  }

  function stream(index) {
    const { bufferView, ...layout } = document.accessors[index];
    return { layout, bytes: viewBytes(bufferView) };
  }

  return {
    triangles: primitives.reduce((sum, primitive) => sum + document.accessors[primitive.indices].count / 3, 0),
    appearance: {
      materials: document.materials,
      textures: document.textures,
      samplers: document.samplers,
      images: document.images.map(({ mimeType, bufferView }) => ({ mimeType, bytes: viewBytes(bufferView) })),
      bindings: primitives.map(({ material, attributes }) => ({
        material,
        uv: stream(attributes.TEXCOORD_0),
        normals: stream(attributes.NORMAL),
      })),
    },
  };
}
