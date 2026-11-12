const GLB_MAGIC = 0x46546c67;
const JSON_CHUNK = 0x4e4f534a;
const BIN_CHUNK = 0x004e4942;

export class InputError extends Error {
  constructor(code, message, status = 400) {
    super(message);
    this.name = "InputError";
    this.code = code;
    this.status = status;
  }
}

export function safeFilename(value) {
  const leaf = String(value || "asset").replaceAll("\\", "/").split("/").pop() || "asset";
  const cleaned = leaf.normalize("NFKC").replace(/[\u0000-\u001f\u007f]/g, "").replace(/[^\p{L}\p{N}._ -]/gu, "_").trim();
  const compact = cleaned.replace(/\s+/g, " ").replace(/^\.+/, "").slice(0, 120);
  return compact || "asset";
}

export function detectAsset(buffer, filename) {
  if (!Buffer.isBuffer(buffer) || buffer.length < 4) throw new InputError("EMPTY_UPLOAD", "The uploaded file is empty or truncated.");
  const name = safeFilename(filename);
  if (buffer.length >= 12 && buffer.readUInt32LE(0) === GLB_MAGIC) {
    if (!name.toLowerCase().endsWith(".glb")) throw new InputError("EXTENSION_MISMATCH", "The GLB content must use a .glb filename.");
    const info = inspectGlb(buffer);
    return { kind: "mesh", name, mimeType: "model/gltf-binary", metadata: info };
  }
  if (buffer.length >= 8 && buffer.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))) {
    if (!/\.png$/i.test(name)) throw new InputError("EXTENSION_MISMATCH", "The PNG image content must use a .png filename.");
    return { kind: "image", name, mimeType: "image/png", metadata: { format: "PNG", bytes: buffer.length } };
  }
  if (buffer[0] === 0xff && buffer[1] === 0xd8 && buffer[2] === 0xff) {
    if (!/\.(jpe?g)$/i.test(name)) throw new InputError("EXTENSION_MISMATCH", "The JPEG image content must use a .jpg or .jpeg filename.");
    return { kind: "image", name, mimeType: "image/jpeg", metadata: { format: "JPEG", bytes: buffer.length } };
  }
  throw new InputError("UNSUPPORTED_FORMAT", "Supported imports are PNG, JPEG, and binary glTF (.glb).");
}

export function inspectGlb(buffer) {
  if (!Buffer.isBuffer(buffer) || buffer.length < 20) throw new InputError("INVALID_GLB", "The file is too short to be a GLB.");
  if (buffer.readUInt32LE(0) !== GLB_MAGIC) throw new InputError("INVALID_GLB", "The GLB magic header is invalid.");
  const version = buffer.readUInt32LE(4);
  const declaredLength = buffer.readUInt32LE(8);
  if (version !== 2) throw new InputError("UNSUPPORTED_GLB_VERSION", `Only glTF binary version 2 is supported; received ${version}.`, 415);
  if (declaredLength !== buffer.length) throw new InputError("INVALID_GLB", "The GLB header length does not match the uploaded bytes.");

  let offset = 12;
  let document = null;
  let binary = null;
  while (offset < declaredLength) {
    if (offset + 8 > declaredLength) throw new InputError("INVALID_GLB", "A GLB chunk header is truncated.");
    const length = buffer.readUInt32LE(offset);
    const type = buffer.readUInt32LE(offset + 4);
    offset += 8;
    if (length % 4 !== 0 || offset + length > declaredLength) throw new InputError("INVALID_GLB", "A GLB chunk has an invalid length.");
    const chunk = buffer.subarray(offset, offset + length);
    if (type === JSON_CHUNK && document === null) {
      try {
        document = JSON.parse(chunk.toString("utf8").replace(/[\u0000\u0020]+$/g, ""));
      } catch {
        throw new InputError("INVALID_GLB_JSON", "The GLB JSON chunk could not be parsed.");
      }
    } else if (type === BIN_CHUNK && binary === null) {
      binary = chunk;
    }
    offset += length;
  }
  if (!document || document.asset?.version !== "2.0") throw new InputError("INVALID_GLB_JSON", "The GLB is missing a valid glTF 2.0 asset declaration.");
  if (!Array.isArray(document.meshes)) throw new InputError("INVALID_GLB_JSON", "The GLB does not contain a meshes array.");
  if (Array.isArray(document.buffers) && document.buffers.some(item => item?.uri)) {
    throw new InputError("EXTERNAL_BUFFER_UNSUPPORTED", "This prototype accepts self-contained GLB files only; external buffer URIs are not imported.", 415);
  }
  if (Array.isArray(document.images) && document.images.some(item => item?.uri)) {
    throw new InputError("EXTERNAL_IMAGE_UNSUPPORTED", "This prototype accepts embedded GLB textures only; external image URIs are not imported.", 415);
  }

  const accessors = Array.isArray(document.accessors) ? document.accessors : [];
  const primitives = document.meshes.flatMap(mesh => Array.isArray(mesh?.primitives) ? mesh.primitives : []);
  const triangleCount = primitives.reduce((sum, primitive) => {
    if ((primitive.mode ?? 4) !== 4) return sum;
    const accessor = Number.isInteger(primitive.indices) ? accessors[primitive.indices] : null;
    const positionAccessor = Number.isInteger(primitive.attributes?.POSITION) ? accessors[primitive.attributes.POSITION] : null;
    const count = accessor?.count ?? positionAccessor?.count ?? 0;
    return sum + Math.floor(count / 3);
  }, 0);
  const positionAccessor = primitives[0]?.attributes?.POSITION;
  const vertexCount = Number.isInteger(positionAccessor) ? accessors[positionAccessor]?.count ?? null : null;
  const textured = (document.materials || []).some(material => Boolean(material?.pbrMetallicRoughness?.baseColorTexture));
  const materialNames = (document.materials || []).map((material, index) => material?.name || `Material ${index + 1}`);
  return {
    format: "GLB",
    version: "2.0",
    meshes: document.meshes.length,
    primitives: primitives.length,
    triangles: triangleCount,
    vertices: vertexCount,
    materials: materialNames.length,
    materialNames,
    hasBaseColorTexture: textured,
    hasBinaryChunk: binary !== null,
    bytes: buffer.length,
    support: inspectSimplifySupport(document, accessors, binary)
  };
}

export function inspectSimplifySupport(document, accessors = document.accessors || [], binary = null) {
  const fail = reason => ({ supported: false, reason });
  if (document.meshes.length !== 1) return fail("The worker supports exactly one mesh.");
  if (document.meshes[0]?.primitives?.length !== 1) return fail("The worker supports exactly one primitive per mesh.");
  if ((document.animations || []).length) return fail("Animated assets are not supported; animation data is preserved only by separate operations.");
  if ((document.skins || []).length) return fail("Skinned assets are not supported by this operation.");
  if ((document.extensionsUsed || []).length || (document.extensionsRequired || []).length) return fail("GLB extensions are not supported by this worker.");
  const primitive = document.meshes[0].primitives[0];
  if ((primitive.mode ?? 4) !== 4) return fail("Only TRIANGLES primitives are supported.");
  if (!Number.isInteger(primitive.indices)) return fail("The worker requires an indexed primitive.");
  const attributes = primitive.attributes || {};
  const expected = ["POSITION", "NORMAL", "TEXCOORD_0"].sort();
  const actual = Object.keys(attributes).sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) return fail(`Only POSITION, NORMAL, and TEXCOORD_0 are supported; found ${actual.join(", ") || "none"}.`);
  if ((document.meshes[0].primitives[0].targets || []).length) return fail("Morph targets are not supported by this worker.");
  for (const key of expected) {
    const accessorId = attributes[key];
    const accessor = accessors[accessorId];
    if (!accessor || accessor.sparse) return fail(`The ${key} accessor is missing or sparse.`);
    if (accessor.componentType !== 5126 || accessor.type !== (key === "TEXCOORD_0" ? "VEC2" : "VEC3")) return fail(`The ${key} accessor must use float ${key === "TEXCOORD_0" ? "VEC2" : "VEC3"} values.`);
    if (!Number.isInteger(accessor.count) || accessor.count < 3) return fail(`The ${key} accessor count is invalid.`);
    if (!Number.isInteger(accessor.bufferView)) return fail(`The ${key} accessor must use a buffer view.`);
  }
  const indexAccessor = accessors[primitive.indices];
  if (!indexAccessor || indexAccessor.sparse || indexAccessor.type !== "SCALAR" || ![5121, 5123, 5125].includes(indexAccessor.componentType)) return fail("The index accessor must be a non-sparse unsigned integer SCALAR.");
  if (!Number.isInteger(indexAccessor.count) || indexAccessor.count < 3 || indexAccessor.count % 3 !== 0) return fail("The index accessor must contain complete triangles.");
  if (binary === null) return fail("The GLB is missing its binary data chunk.");
  if ((document.buffers || []).some(item => item?.uri)) return fail("External buffers are not supported by this worker.");
  if ((document.images || []).some(item => item?.uri)) return fail("External texture images are not supported by this worker.");
  return { supported: true, reason: null };
}

