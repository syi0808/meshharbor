import { readFileSync } from 'node:fs';
import { PNG } from 'pngjs';

const cube = readFileSync(new URL('../../../crates/core/tests/fixtures/quadriflow_cube_triangles.obj', import.meta.url), 'utf8')
  .split(/\r?\n/)
  .map(line => line.trim().split(/\s+/));
const positions = cube.filter(([type]) => type === 'v').map(([, ...xyz]) => xyz.map(Number));
const indices = cube.filter(([type]) => type === 'f').flatMap(([, ...face]) => face.map(index => Number(index) - 1));

export function quadriflowCube({ textured = false } = {}) {
  const data = [];
  const bufferViews = [];
  const accessors = [];
  const addView = (bytes, target) => {
    const byteOffset = data.reduce((size, chunk) => size + chunk.length, 0);
    const padded = Buffer.alloc(Math.ceil(bytes.length / 4) * 4);
    bytes.copy(padded);
    data.push(padded);
    return bufferViews.push({ buffer: 0, byteOffset, byteLength: bytes.length, target }) - 1;
  };
  const addAttribute = (rows, type, bounds) => {
    const bytes = Buffer.alloc(rows.length * rows[0].length * 4);
    rows.flat().forEach((value, index) => bytes.writeFloatLE(value, index * 4));
    const bufferView = addView(bytes, 34962);
    return accessors.push({ bufferView, componentType: 5126, count: rows.length, type, ...bounds }) - 1;
  };

  const vertices = textured ? [...positions, ...positions.slice(0, 4)] : positions;
  const attributes = {
    POSITION: addAttribute(vertices, 'VEC3', { min: [-1, -1, -1], max: [1, 1, 1] }),
  };
  const faces = Buffer.alloc(indices.length * 2);
  indices.forEach((index, corner) => faces.writeUInt16LE(textured && corner < 6 ? index + 8 : index, corner * 2));
  const indexView = addView(faces, 34963);
  const primitive = {
    attributes,
    indices: accessors.push({ bufferView: indexView, componentType: 5123, count: indices.length, type: 'SCALAR' }) - 1,
    mode: 4,
  };
  const document = {
    asset: { version: '2.0' },
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [{ mesh: 0 }],
    meshes: [{ primitives: [primitive] }],
    accessors,
    bufferViews,
  };
  if (textured) {
    attributes.NORMAL = addAttribute(vertices.map(position => position.map(axis => axis / Math.sqrt(3))), 'VEC3');
    attributes.TEXCOORD_0 = addAttribute(vertices.map(([x, y, z], index) => {
      const pair = [(x + 2 * y + 3 * z + 6) / 12, (2 * x - 3 * y + z + 6) / 12];
      return index < 8 ? pair : pair.map(value => 0.1 + 0.8 * value);
    }), 'VEC2');
    primitive.material = 0;
    document.images = [[255, 40, 16, 255], [0, 128, 248, 255]].map(color => {
      const image = new PNG({ width: 2, height: 2 });
      image.data = Buffer.from([...color, ...color, ...color, ...color]);
      return { bufferView: addView(PNG.sync.write(image)), mimeType: 'image/png' };
    });
    document.textures = [{ source: 0 }, { source: 1 }];
    document.materials = [{ pbrMetallicRoughness: { baseColorTexture: { index: 0 }, metallicRoughnessTexture: { index: 1 } } }];
  }
  const binary = Buffer.concat(data);
  document.buffers = [{ byteLength: binary.length }];
  const json = Buffer.from(JSON.stringify(document));
  const aligned = Buffer.alloc(Math.ceil(json.length / 4) * 4, 0x20);
  json.copy(aligned);
  const header = Buffer.alloc(20);
  header.write('glTF');
  header.writeUInt32LE(2, 4);
  header.writeUInt32LE(header.length + aligned.length + 8 + binary.length, 8);
  header.writeUInt32LE(aligned.length, 12);
  header.write('JSON', 16);
  const binaryHeader = Buffer.alloc(8);
  binaryHeader.writeUInt32LE(binary.length);
  binaryHeader.write('BIN\0', 4);
  return Buffer.concat([header, aligned, binaryHeader, binary]);
}
