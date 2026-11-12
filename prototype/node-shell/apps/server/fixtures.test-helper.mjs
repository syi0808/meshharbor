export function createGridGlb(size = 4) {
  const positions = [];
  const normals = [];
  const uv = [];
  const indices = [];
  for (let y = 0; y <= size; y++) {
    for (let x = 0; x <= size; x++) {
      positions.push(x / size, y / size, Math.sin(x * 0.4) * Math.sin(y * 0.3) * 0.03);
      normals.push(0, 0, 1);
      uv.push(x / size, y / size);
    }
  }
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const a = y * (size + 1) + x;
      const b = a + 1;
      const d = a + size + 1;
      const c = d + 1;
      indices.push(a, b, c, a, c, d);
    }
  }
  const posBytes = Buffer.alloc(positions.length * 4);
  positions.forEach((value, index) => posBytes.writeFloatLE(value, index * 4));
  const normalBytes = Buffer.alloc(normals.length * 4);
  normals.forEach((value, index) => normalBytes.writeFloatLE(value, index * 4));
  const uvBytes = Buffer.alloc(uv.length * 4);
  uv.forEach((value, index) => uvBytes.writeFloatLE(value, index * 4));
  const indexBytes = Buffer.alloc(indices.length * 4);
  indices.forEach((value, index) => indexBytes.writeUInt32LE(value, index * 4));
  const parts = [posBytes, normalBytes, uvBytes, indexBytes];
  const offsets = [];
  let offset = 0;
  for (const part of parts) { offsets.push(offset); offset += part.length; }
  const binary = Buffer.concat(parts);
  const document = {
    asset: { version: "2.0", generator: "MeshHarbor tests" },
    buffers: [{ byteLength: binary.length }],
    bufferViews: parts.map((part, index) => ({ buffer: 0, byteOffset: offsets[index], byteLength: part.length, target: index === 3 ? 34963 : 34962 })),
    accessors: [
      { bufferView: 0, componentType: 5126, count: positions.length / 3, type: "VEC3", min: [0, 0, -0.03], max: [1, 1, 0.03] },
      { bufferView: 1, componentType: 5126, count: normals.length / 3, type: "VEC3" },
      { bufferView: 2, componentType: 5126, count: uv.length / 2, type: "VEC2" },
      { bufferView: 3, componentType: 5125, count: indices.length, type: "SCALAR" }
    ],
    images: [],
    textures: [],
    materials: [{ name: "Harbor clay", pbrMetallicRoughness: { baseColorFactor: [0.72, 0.42, 0.28, 1], metallicFactor: 0.05, roughnessFactor: 0.76 } }],
    meshes: [{ primitives: [{ attributes: { POSITION: 0, NORMAL: 1, TEXCOORD_0: 2 }, indices: 3, material: 0, mode: 4 }] }],
    nodes: [{ mesh: 0 }],
    scenes: [{ nodes: [0] }],
    scene: 0
  };
  let json = Buffer.from(JSON.stringify(document), "utf8");
  const jsonPadding = (4 - (json.length % 4)) % 4;
  if (jsonPadding) json = Buffer.concat([json, Buffer.alloc(jsonPadding, 0x20)]);
  const binPadding = (4 - (binary.length % 4)) % 4;
  const paddedBinary = binPadding ? Buffer.concat([binary, Buffer.alloc(binPadding)]) : binary;
  const output = Buffer.alloc(12 + 8 + json.length + 8 + paddedBinary.length);
  output.writeUInt32LE(0x46546c67, 0);
  output.writeUInt32LE(2, 4);
  output.writeUInt32LE(output.length, 8);
  output.writeUInt32LE(json.length, 12);
  output.writeUInt32LE(0x4e4f534a, 16);
  json.copy(output, 20);
  const binHeader = 20 + json.length;
  output.writeUInt32LE(paddedBinary.length, binHeader);
  output.writeUInt32LE(0x004e4942, binHeader + 4);
  paddedBinary.copy(output, binHeader + 8);
  return output;
}
