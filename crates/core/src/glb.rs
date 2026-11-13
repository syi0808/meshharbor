use meshopt::{SimplifyOptions, SimplifyVertex, VertexDataAdapter};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::{
    AssetKind, CoreError, RetopologyOptions, RetopologyReport, remesh_triangles_in_worker,
};

const GLB_MAGIC: &[u8; 4] = b"glTF";
const JSON_CHUNK: u32 = 0x4e4f_534a;
const BIN_CHUNK: u32 = 0x004e_4942;

#[derive(Debug, Clone)]
pub struct AssetDescriptor {
    pub kind: AssetKind,
    pub mime_type: &'static str,
    pub metadata: Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SimplificationReport {
    pub triangles_before: u64,
    pub triangles_after: u64,
    pub target_triangles: u64,
    pub geometric_error: f32,
    pub optimized_primitives: u64,
    pub preserved: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MeshCleanupReport {
    pub triangles_before: u64,
    pub triangles_after: u64,
    pub repeated_index_triangles_removed: u64,
    pub zero_area_triangles_removed: u64,
    pub duplicate_triangles_removed: u64,
    pub modified_primitives: u64,
    pub preserved: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenderOptimizationReport {
    pub triangles: u64,
    pub processed_primitives: u64,
    pub reordered_primitives: u64,
    pub skipped_translucent_primitives: u64,
    pub vertex_cache_size: usize,
    pub vertex_cache_misses_before: u64,
    pub vertex_cache_misses_after: u64,
    pub overdraw_threshold: f32,
    pub preserved: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompressionReport {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub compressed_buffer_views: u64,
    pub uncompressed_buffer_views: u64,
    pub bytes_saved: u64,
    pub extension: String,
    pub extension_required: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
struct GlbFile {
    document: Value,
    binary: Option<Vec<u8>>,
    other_chunks: Vec<(u32, Vec<u8>)>,
}

impl GlbFile {
    fn parse(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() < 20 || bytes.get(..4) != Some(GLB_MAGIC.as_slice()) {
            return Err(CoreError::InvalidInput(
                "GLB header is missing or truncated.".into(),
            ));
        }
        let version = read_u32(bytes, 4)?;
        let declared_length = usize::try_from(read_u32(bytes, 8)?)
            .map_err(|_| CoreError::InvalidInput("GLB length is not representable.".into()))?;
        if version != 2 || declared_length != bytes.len() {
            return Err(CoreError::InvalidInput(
                "Only complete glTF binary version 2 files are supported.".into(),
            ));
        }

        let mut cursor = 12usize;
        let mut document = None;
        let mut binary = None;
        let mut other_chunks = Vec::new();
        let mut chunk_index = 0usize;
        while cursor < bytes.len() {
            let header_end = cursor
                .checked_add(8)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| CoreError::InvalidInput("GLB chunk header is truncated.".into()))?;
            let chunk_length = usize::try_from(read_u32(bytes, cursor)?).map_err(|_| {
                CoreError::InvalidInput("GLB chunk length is not representable.".into())
            })?;
            let chunk_type = read_u32(bytes, cursor + 4)?;
            if chunk_length % 4 != 0 {
                return Err(CoreError::InvalidInput(
                    "GLB chunk length must be 4-byte aligned.".into(),
                ));
            }
            let chunk_start = header_end;
            let chunk_end = chunk_start
                .checked_add(chunk_length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| CoreError::InvalidInput("GLB chunk exceeds file bounds.".into()))?;
            let chunk = &bytes[chunk_start..chunk_end];
            if chunk_index == 0 && chunk_type != JSON_CHUNK {
                return Err(CoreError::InvalidInput(
                    "GLB must begin with a JSON chunk.".into(),
                ));
            }
            match chunk_type {
                JSON_CHUNK if document.is_none() && chunk_index == 0 => {
                    document = Some(serde_json::from_slice(chunk).map_err(|_| {
                        CoreError::InvalidInput("GLB JSON chunk is invalid.".into())
                    })?);
                }
                JSON_CHUNK => {
                    return Err(CoreError::InvalidInput(
                        "GLB contains more than one JSON chunk.".into(),
                    ));
                }
                BIN_CHUNK if binary.is_none() => binary = Some(chunk.to_vec()),
                BIN_CHUNK => {
                    return Err(CoreError::InvalidInput(
                        "GLB contains more than one BIN chunk.".into(),
                    ));
                }
                unknown => other_chunks.push((unknown, chunk.to_vec())),
            }
            cursor = chunk_end;
            chunk_index += 1;
        }
        let document =
            document.ok_or_else(|| CoreError::InvalidInput("GLB JSON chunk is missing.".into()))?;
        Ok(Self {
            document,
            binary,
            other_chunks,
        })
    }

    fn write(&self) -> Result<Vec<u8>, CoreError> {
        let mut chunks: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut json_bytes = serde_json::to_vec(&self.document)?;
        pad_vec(&mut json_bytes, b' ');
        chunks.push((JSON_CHUNK, json_bytes));
        if let Some(binary) = &self.binary {
            let mut binary_bytes = binary.clone();
            pad_vec(&mut binary_bytes, 0);
            chunks.push((BIN_CHUNK, binary_bytes));
        }
        chunks.extend(self.other_chunks.iter().cloned());

        let total = chunks
            .iter()
            .try_fold(12usize, |sum, (_, chunk)| {
                sum.checked_add(8)?.checked_add(chunk.len())
            })
            .ok_or_else(|| {
                CoreError::InvalidInput("Output GLB would exceed supported file size.".into())
            })?;
        let total_u32 = u32::try_from(total).map_err(|_| {
            CoreError::InvalidInput("Output GLB exceeds the 4 GiB format limit.".into())
        })?;
        let mut output = Vec::with_capacity(total);
        output.extend_from_slice(GLB_MAGIC);
        output.extend_from_slice(&2u32.to_le_bytes());
        output.extend_from_slice(&total_u32.to_le_bytes());
        for (kind, chunk) in chunks {
            let length = u32::try_from(chunk.len()).map_err(|_| {
                CoreError::InvalidInput("GLB chunk exceeds the 4 GiB format limit.".into())
            })?;
            output.extend_from_slice(&length.to_le_bytes());
            output.extend_from_slice(&kind.to_le_bytes());
            output.extend_from_slice(&chunk);
        }
        Ok(output)
    }
}

pub fn inspect_asset(filename: &str, bytes: &[u8]) -> Result<AssetDescriptor, CoreError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(AssetDescriptor {
            kind: AssetKind::Image,
            mime_type: "image/png",
            metadata: json!({ "format": "png" }),
        });
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok(AssetDescriptor {
            kind: AssetKind::Image,
            mime_type: "image/jpeg",
            metadata: json!({ "format": "jpeg" }),
        });
    }
    if bytes.get(..4) == Some(GLB_MAGIC.as_slice()) {
        return inspect_glb(bytes);
    }

    let extension = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    Err(CoreError::InvalidInput(format!(
        "Unsupported or malformed asset '{}'. Accepted inputs are PNG, JPEG, and GLB.",
        if extension.is_empty() {
            "file"
        } else {
            extension.as_str()
        }
    )))
}

fn inspect_glb(bytes: &[u8]) -> Result<AssetDescriptor, CoreError> {
    let glb = GlbFile::parse(bytes)?;
    let accessors = array(&glb.document, "accessors");
    let meshes = array(&glb.document, "meshes");
    let mut primitive_count = 0u64;
    let mut triangles = 0u64;
    for mesh in meshes {
        for primitive in array(mesh, "primitives") {
            primitive_count += 1;
            let mode = primitive.get("mode").and_then(Value::as_u64).unwrap_or(4);
            if mode == 4 {
                let count = primitive
                    .get("indices")
                    .and_then(Value::as_u64)
                    .and_then(|index| accessors.get(index as usize))
                    .and_then(|accessor| accessor.get("count"))
                    .and_then(Value::as_u64)
                    .or_else(|| {
                        primitive
                            .get("attributes")
                            .and_then(|attributes| attributes.get("POSITION"))
                            .and_then(Value::as_u64)
                            .and_then(|index| accessors.get(index as usize))
                            .and_then(|accessor| accessor.get("count"))
                            .and_then(Value::as_u64)
                    })
                    .unwrap_or(0);
                triangles += count / 3;
            }
        }
    }
    let mut metadata = Map::new();
    metadata.insert("format".into(), json!("glb"));
    metadata.insert("version".into(), json!(2));
    metadata.insert("mesh_count".into(), json!(meshes.len()));
    metadata.insert("primitive_count".into(), json!(primitive_count));
    metadata.insert("triangles".into(), json!(triangles));
    metadata.insert(
        "material_count".into(),
        json!(array(&glb.document, "materials").len()),
    );
    metadata.insert(
        "has_animations".into(),
        json!(!array(&glb.document, "animations").is_empty()),
    );
    Ok(AssetDescriptor {
        kind: AssetKind::Mesh,
        mime_type: "model/gltf-binary",
        metadata: Value::Object(metadata),
    })
}

/// Compresses supported geometry bufferViews with the Khronos
/// KHR_meshopt_compression extension. The output requires consumers to support
/// that extension; uncompressed fallback geometry is intentionally not copied.
pub fn compress_mesh_glb(bytes: &[u8]) -> Result<(Vec<u8>, CompressionReport), CoreError> {
    let mut glb = GlbFile::parse(bytes)?;
    validate_static_glb(&glb.document)?;
    let original_binary = glb
        .binary
        .as_ref()
        .ok_or_else(|| CoreError::InvalidInput("GLB has no embedded BIN chunk.".into()))?;
    validate_single_embedded_buffer(&glb.document, original_binary.len())?;
    let declared_original_length = array(&glb.document, "buffers")[0]
        .get("byteLength")
        .and_then(Value::as_u64)
        .and_then(|length| usize::try_from(length).ok())
        .ok_or_else(|| {
            CoreError::InvalidInput("Embedded buffer byteLength is missing or invalid.".into())
        })?;
    if array(&glb.document, "bufferViews")
        .iter()
        .any(|view| view.get("extensions").is_some())
        || array(&glb.document, "buffers")
            .iter()
            .any(|buffer| buffer.get("extensions").is_some())
    {
        return Err(CoreError::OperationUnavailable(
            "Compression does not rewrite bufferView or buffer extensions.".into(),
        ));
    }

    let buffer_views = array(&glb.document, "bufferViews").to_vec();
    let accessors = array(&glb.document, "accessors").to_vec();
    let mut accessor_roles: HashMap<usize, u8> = HashMap::new();
    for (mesh_index, mesh) in array(&glb.document, "meshes").iter().enumerate() {
        for (primitive_index, primitive) in array(mesh, "primitives").iter().enumerate() {
            let mode = primitive.get("mode").and_then(Value::as_u64).unwrap_or(4);
            if mode != 4 {
                return Err(CoreError::OperationUnavailable(format!(
                    "Mesh {mesh_index} primitive {primitive_index} uses primitive mode {mode}; compression currently supports indexed triangles only."
                )));
            }
            let index_accessor = primitive
                .get("indices")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    CoreError::OperationUnavailable(
                        "Compression requires indexed triangle primitives.".into(),
                    )
                })? as usize;
            mark_accessor_role(&mut accessor_roles, index_accessor, 2)?;
            let attributes = primitive
                .get("attributes")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    CoreError::InvalidInput("Triangle primitive has no attributes.".into())
                })?;
            for accessor in attributes.values() {
                let accessor_index = accessor.as_u64().ok_or_else(|| {
                    CoreError::InvalidInput("Primitive attribute accessor is invalid.".into())
                })? as usize;
                mark_accessor_role(&mut accessor_roles, accessor_index, 1)?;
            }
        }
    }

    let mut accessors_by_view = vec![Vec::<usize>::new(); buffer_views.len()];
    for (accessor_index, accessor) in accessors.iter().enumerate() {
        if let Some(view_index) = accessor.get("bufferView").and_then(Value::as_u64) {
            let view_index = view_index as usize;
            let by_view = accessors_by_view.get_mut(view_index).ok_or_else(|| {
                CoreError::InvalidInput("Accessor references a missing bufferView.".into())
            })?;
            by_view.push(accessor_index);
        }
    }
    let image_views: HashSet<usize> = array(&glb.document, "images")
        .iter()
        .filter_map(|image| image.get("bufferView").and_then(Value::as_u64))
        .map(|view| view as usize)
        .collect();
    let mut sparse_views = HashSet::new();
    for accessor in &accessors {
        if let Some(sparse) = accessor.get("sparse") {
            for sparse_view in ["indices", "values"]
                .iter()
                .copied()
                .filter_map(|key| sparse.get(key))
                .filter_map(|entry| entry.get("bufferView").and_then(Value::as_u64))
            {
                sparse_views.insert(sparse_view as usize);
            }
        }
    }

    let mut candidates = BTreeMap::<usize, MeshoptCandidate>::new();
    let mut warnings = Vec::new();
    for (view_index, view) in buffer_views.iter().enumerate() {
        if image_views.contains(&view_index)
            || sparse_views.contains(&view_index)
            || accessors_by_view[view_index].is_empty()
        {
            continue;
        }
        if accessors_by_view[view_index]
            .iter()
            .any(|accessor_index| !accessor_roles.contains_key(accessor_index))
        {
            continue;
        }
        let roles: HashSet<u8> = accessors_by_view[view_index]
            .iter()
            .filter_map(|accessor_index| accessor_roles.get(accessor_index).copied())
            .collect();
        if roles.len() != 1 {
            continue;
        }
        let (start, byte_length) =
            buffer_view_range(&glb.document, view_index, declared_original_length)?;
        let role = *roles.iter().next().expect("one accessor role");
        let candidate = if role == 1 {
            encode_attribute_buffer_view(
                view_index,
                view,
                &accessors_by_view[view_index],
                &accessors,
                start,
                byte_length,
                original_binary,
            )?
        } else {
            encode_triangle_buffer_view(
                view_index,
                view,
                &accessors_by_view[view_index],
                &accessors,
                start,
                byte_length,
                original_binary,
            )?
        };
        if let Some(candidate) = candidate {
            if candidate.encoded.len() < candidate.byte_length {
                candidates.insert(view_index, candidate);
            } else {
                warnings.push(format!(
                    "Kept bufferView {view_index} uncompressed because meshoptimizer did not reduce its size."
                ));
            }
        }
    }
    if candidates.is_empty() {
        return Err(CoreError::OperationUnavailable(
            "No supported mesh bufferView could be compressed with a net payload reduction.".into(),
        ));
    }

    let mut compressed_binary = Vec::new();
    for view_index in 0..buffer_views.len() {
        if candidates.contains_key(&view_index) {
            continue;
        }
        let (start, byte_length) =
            buffer_view_range(&glb.document, view_index, declared_original_length)?;
        while !compressed_binary.len().is_multiple_of(4) {
            compressed_binary.push(0);
        }
        let new_offset = compressed_binary.len();
        let end = start
            .checked_add(byte_length)
            .ok_or_else(|| CoreError::InvalidInput("bufferView range overflowed.".into()))?;
        compressed_binary.extend_from_slice(&original_binary[start..end]);
        glb.document["bufferViews"][view_index]["buffer"] = json!(0);
        glb.document["bufferViews"][view_index]["byteOffset"] = json!(new_offset);
    }

    let mut compressed_view_count = 0u64;
    for (view_index, candidate) in candidates {
        while !compressed_binary.len().is_multiple_of(4) {
            compressed_binary.push(0);
        }
        let encoded_offset = compressed_binary.len();
        compressed_binary.extend_from_slice(&candidate.encoded);
        compressed_view_count += 1;
        let view = &mut glb.document["bufferViews"][view_index];
        view["buffer"] = json!(1);
        view["byteOffset"] = json!(candidate.original_offset);
        view["extensions"]["KHR_meshopt_compression"] = json!({
            "buffer": 0,
            "byteOffset": encoded_offset,
            "byteLength": candidate.encoded.len(),
            "byteStride": candidate.byte_stride,
            "count": candidate.count,
            "mode": candidate.mode,
        });
    }

    ensure_extension(
        &mut glb.document,
        "extensionsUsed",
        "KHR_meshopt_compression",
    );
    ensure_extension(
        &mut glb.document,
        "extensionsRequired",
        "KHR_meshopt_compression",
    );
    let buffers = glb
        .document
        .get_mut("buffers")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CoreError::InvalidInput("GLB buffer metadata is missing.".into()))?;
    let original_buffer = buffers
        .first()
        .cloned()
        .ok_or_else(|| CoreError::InvalidInput("GLB buffer metadata is missing.".into()))?;
    let mut output_buffer = original_buffer.clone();
    output_buffer["byteLength"] = json!(compressed_binary.len());
    let mut fallback_buffer = original_buffer;
    fallback_buffer["byteLength"] = json!(declared_original_length);
    fallback_buffer["extensions"]["KHR_meshopt_compression"] = json!({ "fallback": true });
    *buffers = vec![output_buffer, fallback_buffer];
    glb.binary = Some(compressed_binary);
    let encoded = glb.write()?;
    let output_bytes = encoded.len();
    if output_bytes >= bytes.len() {
        return Err(CoreError::OperationUnavailable(
            "KHR_meshopt_compression payloads were generated, but extension metadata outweighed the savings; source asset was left unchanged.".into(),
        ));
    }
    Ok((
        encoded,
        CompressionReport {
            input_bytes: bytes.len() as u64,
            output_bytes: output_bytes as u64,
            compressed_buffer_views: compressed_view_count,
            uncompressed_buffer_views: buffer_views.len() as u64 - compressed_view_count,
            bytes_saved: bytes.len() as u64 - output_bytes as u64,
            extension: "KHR_meshopt_compression".into(),
            extension_required: true,
            warnings: {
                warnings.push(
                    "This GLB requires KHR_meshopt_compression support; uncompressed geometry fallback data is not embedded.".into(),
                );
                warnings
            },
        },
    ))
}

#[derive(Debug, Clone)]
struct MeshoptCandidate {
    original_offset: usize,
    byte_length: usize,
    byte_stride: usize,
    count: usize,
    mode: &'static str,
    encoded: Vec<u8>,
}

fn mark_accessor_role(
    roles: &mut HashMap<usize, u8>,
    accessor_index: usize,
    role: u8,
) -> Result<(), CoreError> {
    if let Some(previous) = roles.insert(accessor_index, role)
        && previous != role
    {
        return Err(CoreError::OperationUnavailable(
            "An accessor is used as both vertex attributes and indices.".into(),
        ));
    }
    Ok(())
}

fn buffer_view_range(
    document: &Value,
    view_index: usize,
    binary_len: usize,
) -> Result<(usize, usize), CoreError> {
    let view = array(document, "bufferViews")
        .get(view_index)
        .ok_or_else(|| CoreError::InvalidInput("bufferView does not exist.".into()))?;
    if view.get("buffer").and_then(Value::as_u64).unwrap_or(0) != 0 {
        return Err(CoreError::OperationUnavailable(
            "Compression supports bufferViews that reference the embedded GLB buffer only.".into(),
        ));
    }
    let offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let length = view
        .get("byteLength")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("bufferView byteLength is missing.".into()))?
        as usize;
    offset
        .checked_add(length)
        .filter(|end| *end <= binary_len)
        .ok_or_else(|| {
            CoreError::InvalidInput("bufferView exceeds the embedded GLB buffer.".into())
        })?;
    Ok((offset, length))
}

fn encode_attribute_buffer_view(
    _view_index: usize,
    view: &Value,
    accessor_indices: &[usize],
    accessors: &[Value],
    start: usize,
    byte_length: usize,
    binary: &[u8],
) -> Result<Option<MeshoptCandidate>, CoreError> {
    let Some(first_accessor) = accessor_indices
        .first()
        .and_then(|index| accessors.get(*index))
    else {
        return Ok(None);
    };
    let count = first_accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("Attribute accessor count is missing.".into()))?
        as usize;
    if count == 0
        || accessor_indices.iter().any(|index| {
            accessors
                .get(*index)
                .and_then(|accessor| accessor.get("count"))
                .and_then(Value::as_u64)
                != Some(count as u64)
        })
    {
        return Ok(None);
    }
    let explicit_stride = view.get("byteStride").and_then(Value::as_u64);
    if accessor_indices.len() > 1 && explicit_stride.is_none() {
        return Ok(None);
    }
    let stride = explicit_stride
        .map(|stride| stride as usize)
        .unwrap_or_else(|| byte_length / count);
    if !(4..=256).contains(&stride)
        || !stride.is_multiple_of(4)
        || stride.checked_mul(count) != Some(byte_length)
    {
        return Ok(None);
    }
    for accessor_index in accessor_indices {
        let accessor = &accessors[*accessor_index];
        let Some(component_type) = accessor.get("componentType").and_then(Value::as_u64) else {
            return Ok(None);
        };
        let component_size = component_size(component_type)?;
        let Some(accessor_type) = accessor.get("type").and_then(Value::as_str) else {
            return Ok(None);
        };
        let dimensions = match accessor_type {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            _ => return Ok(None),
        };
        let element_length = component_size * dimensions;
        let accessor_offset = accessor
            .get("byteOffset")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        if accessor_offset
            .checked_add(element_length)
            .is_none_or(|end| end > stride)
        {
            return Ok(None);
        }
        if explicit_stride.is_none() && (element_length != stride || accessor_offset != 0) {
            return Ok(None);
        }
    }
    let Some(encoded) =
        encode_vertex_buffer_with_stride(&binary[start..start + byte_length], count, stride)?
    else {
        return Ok(None);
    };
    Ok(Some(MeshoptCandidate {
        original_offset: start,
        byte_length,
        byte_stride: stride,
        count,
        mode: "ATTRIBUTES",
        encoded,
    }))
}

fn encode_triangle_buffer_view(
    view_index: usize,
    _view: &Value,
    accessor_indices: &[usize],
    accessors: &[Value],
    start: usize,
    byte_length: usize,
    binary: &[u8],
) -> Result<Option<MeshoptCandidate>, CoreError> {
    if accessor_indices.len() != 1 {
        return Ok(None);
    }
    let accessor = &accessors[accessor_indices[0]];
    let component_type = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("Index componentType is missing.".into()))?;
    let byte_stride = match component_type {
        5123 => 2usize,
        5125 => 4,
        _ => return Ok(None),
    };
    let count = accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("Index accessor count is missing.".into()))?
        as usize;
    if count == 0
        || !count.is_multiple_of(3)
        || accessor.get("type").and_then(Value::as_str) != Some("SCALAR")
        || accessor.get("sparse").is_some()
        || accessor
            .get("byteOffset")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            != 0
        || count.checked_mul(byte_stride) != Some(byte_length)
    {
        return Ok(None);
    }
    let mut indices = Vec::with_capacity(count);
    for chunk in binary[start..start + byte_length].chunks_exact(byte_stride) {
        let index = if byte_stride == 2 {
            u16::from_le_bytes([chunk[0], chunk[1]]) as u32
        } else {
            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        };
        indices.push(index);
    }
    let vertex_count = indices
        .iter()
        .max()
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .and_then(|count| usize::try_from(count).ok())
        .ok_or_else(|| {
            CoreError::InvalidInput("Index vertex count is not representable.".into())
        })?;
    let encoded = meshopt::encode_index_buffer(&indices, vertex_count).map_err(|error| {
        CoreError::OperationUnavailable(format!(
            "meshoptimizer could not encode bufferView {view_index}: {error}"
        ))
    })?;
    Ok(Some(MeshoptCandidate {
        original_offset: start,
        byte_length,
        byte_stride,
        count,
        mode: "TRIANGLES",
        encoded,
    }))
}

fn encode_vertex_buffer_with_stride(
    bytes: &[u8],
    count: usize,
    stride: usize,
) -> Result<Option<Vec<u8>>, CoreError> {
    macro_rules! encode_stride {
        ($size:literal) => {{
            let records: Vec<[u8; $size]> = bytes
                .chunks_exact($size)
                .map(|chunk| {
                    let mut record = [0u8; $size];
                    record.copy_from_slice(chunk);
                    record
                })
                .collect();
            meshopt::encode_vertex_buffer(&records)
                .map(Some)
                .map_err(|error| {
                    CoreError::OperationUnavailable(format!(
                        "meshoptimizer could not encode vertex buffer: {error}"
                    ))
                })
        }};
    }
    if bytes.len() != count.saturating_mul(stride) {
        return Ok(None);
    }
    match stride {
        4 => encode_stride!(4),
        8 => encode_stride!(8),
        12 => encode_stride!(12),
        16 => encode_stride!(16),
        20 => encode_stride!(20),
        24 => encode_stride!(24),
        28 => encode_stride!(28),
        32 => encode_stride!(32),
        36 => encode_stride!(36),
        40 => encode_stride!(40),
        44 => encode_stride!(44),
        48 => encode_stride!(48),
        52 => encode_stride!(52),
        56 => encode_stride!(56),
        60 => encode_stride!(60),
        64 => encode_stride!(64),
        68 => encode_stride!(68),
        72 => encode_stride!(72),
        76 => encode_stride!(76),
        80 => encode_stride!(80),
        84 => encode_stride!(84),
        88 => encode_stride!(88),
        92 => encode_stride!(92),
        96 => encode_stride!(96),
        100 => encode_stride!(100),
        104 => encode_stride!(104),
        108 => encode_stride!(108),
        112 => encode_stride!(112),
        116 => encode_stride!(116),
        120 => encode_stride!(120),
        124 => encode_stride!(124),
        128 => encode_stride!(128),
        132 => encode_stride!(132),
        136 => encode_stride!(136),
        140 => encode_stride!(140),
        144 => encode_stride!(144),
        148 => encode_stride!(148),
        152 => encode_stride!(152),
        156 => encode_stride!(156),
        160 => encode_stride!(160),
        164 => encode_stride!(164),
        168 => encode_stride!(168),
        172 => encode_stride!(172),
        176 => encode_stride!(176),
        180 => encode_stride!(180),
        184 => encode_stride!(184),
        188 => encode_stride!(188),
        192 => encode_stride!(192),
        196 => encode_stride!(196),
        200 => encode_stride!(200),
        204 => encode_stride!(204),
        208 => encode_stride!(208),
        212 => encode_stride!(212),
        216 => encode_stride!(216),
        220 => encode_stride!(220),
        224 => encode_stride!(224),
        228 => encode_stride!(228),
        232 => encode_stride!(232),
        236 => encode_stride!(236),
        240 => encode_stride!(240),
        244 => encode_stride!(244),
        248 => encode_stride!(248),
        252 => encode_stride!(252),
        256 => encode_stride!(256),
        _ => Ok(None),
    }
}

fn ensure_extension(document: &mut Value, list_name: &str, extension: &str) {
    if !document[list_name].is_array() {
        document[list_name] = json!([]);
    }
    let list = document[list_name]
        .as_array_mut()
        .expect("extension list is an array");
    if !list.iter().any(|item| item.as_str() == Some(extension)) {
        list.push(json!(extension));
    }
}

/// Removes repeated-index and exactly zero-area triangles plus duplicate
/// same-winding faces from indexed, static GLB triangle primitives. It does
/// not weld vertices, compact vertex streams, or cross material/seam boundaries.
pub fn cleanup_glb(bytes: &[u8]) -> Result<(Vec<u8>, MeshCleanupReport), CoreError> {
    let mut glb = GlbFile::parse(bytes)?;
    validate_static_glb(&glb.document)?;
    let binary = glb
        .binary
        .as_mut()
        .ok_or_else(|| CoreError::InvalidInput("GLB has no embedded BIN chunk.".into()))?;
    validate_single_embedded_buffer(&glb.document, binary.len())?;

    let meshes = glb
        .document
        .get("meshes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut total_before = 0u64;
    let mut repeated_removed = 0u64;
    let mut zero_area_removed = 0u64;
    let mut duplicate_removed = 0u64;
    let mut per_primitive = Vec::new();

    for (mesh_index, mesh) in meshes.iter().enumerate() {
        for (primitive_index, primitive) in array(mesh, "primitives").iter().enumerate() {
            let mode = primitive.get("mode").and_then(Value::as_u64).unwrap_or(4);
            if mode != 4 {
                return Err(CoreError::OperationUnavailable(format!(
                    "Mesh {mesh_index} primitive {primitive_index} uses primitive mode {mode}; cleanup supports triangles only."
                )));
            }
            let indices = read_primitive_indices(&glb.document, binary, primitive)?;
            total_before = total_before
                .checked_add((indices.len() / 3) as u64)
                .ok_or_else(|| CoreError::InvalidInput("Triangle count overflowed.".into()))?;
            let positions = read_cleanup_positions(&glb.document, binary, primitive)?;
            if indices
                .iter()
                .any(|index| *index as usize >= positions.len())
            {
                return Err(CoreError::InvalidInput(
                    "Primitive index refers to a vertex outside POSITION.".into(),
                ));
            }

            let mut seen_faces = HashSet::with_capacity(indices.len() / 3);
            let mut cleaned = Vec::with_capacity(indices.len());
            for face in indices.chunks_exact(3) {
                let [a, b, c] = [face[0], face[1], face[2]];
                if a == b || b == c || c == a {
                    repeated_removed += 1;
                    continue;
                }
                if triangle_has_zero_area(
                    positions[a as usize],
                    positions[b as usize],
                    positions[c as usize],
                ) {
                    zero_area_removed += 1;
                    continue;
                }
                if !seen_faces.insert(canonical_oriented_triangle(a, b, c)) {
                    duplicate_removed += 1;
                    continue;
                }
                cleaned.extend_from_slice(&[a, b, c]);
            }
            if cleaned.is_empty() {
                return Err(CoreError::OperationUnavailable(format!(
                    "Cleanup would remove every triangle from mesh {mesh_index} primitive {primitive_index}; the source asset was left unchanged."
                )));
            }
            if cleaned.len() != indices.len() {
                per_primitive.push((mesh_index, primitive_index, cleaned));
            }
        }
    }
    if total_before == 0 {
        return Err(CoreError::InvalidInput(
            "GLB contains no indexed triangle primitives to clean.".into(),
        ));
    }

    let modified_primitives = per_primitive.len() as u64;
    for (mesh_index, primitive_index, indices) in per_primitive {
        let accessor_index = append_index_accessor(&mut glb.document, binary, &indices)?;
        glb.document["meshes"][mesh_index]["primitives"][primitive_index]["indices"] =
            json!(accessor_index);
    }

    let total_removed = repeated_removed + zero_area_removed + duplicate_removed;
    let encoded = if total_removed == 0 {
        bytes.to_vec()
    } else {
        let buffers = glb
            .document
            .get_mut("buffers")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| CoreError::InvalidInput("GLB buffer metadata is missing.".into()))?;
        buffers[0]["byteLength"] = json!(binary.len());
        glb.write()?
    };
    Ok((
        encoded,
        MeshCleanupReport {
            triangles_before: total_before,
            triangles_after: total_before - total_removed,
            repeated_index_triangles_removed: repeated_removed,
            zero_area_triangles_removed: zero_area_removed,
            duplicate_triangles_removed: duplicate_removed,
            modified_primitives,
            preserved: vec![
                "vertex streams, including UV seams".into(),
                "materials and texture references".into(),
                "mesh and node hierarchy".into(),
                "opposite-winding faces".into(),
            ],
            warnings: vec![
                "Cleanup intentionally does not weld vertices or compact unused vertex data.".into(),
                "Zero-area detection is exact in decoded FLOAT POSITION space; near-degenerate faces are retained.".into(),
            ],
        },
    ))
}

/// Reorders opaque and alpha-masked indexed triangle primitives for the GPU
/// vertex cache and overdraw, without changing vertex streams or triangle count.
/// Translucent primitives are preserved byte-for-byte because their order can
/// affect blending.
pub fn optimize_render_glb(
    bytes: &[u8],
    overdraw_threshold: f32,
) -> Result<(Vec<u8>, RenderOptimizationReport), CoreError> {
    const CACHE_SIZE: usize = 16;
    if !overdraw_threshold.is_finite() || !(1.0..=1.5).contains(&overdraw_threshold) {
        return Err(CoreError::InvalidInput(
            "overdraw_threshold must be finite and between 1.0 and 1.5.".into(),
        ));
    }
    let mut glb = GlbFile::parse(bytes)?;
    validate_static_glb(&glb.document)?;
    let binary = glb
        .binary
        .as_mut()
        .ok_or_else(|| CoreError::InvalidInput("GLB has no embedded BIN chunk.".into()))?;
    validate_single_embedded_buffer(&glb.document, binary.len())?;
    let meshes = glb
        .document
        .get("meshes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut triangles = 0u64;
    let mut processed_primitives = 0u64;
    let mut reordered_primitives = 0u64;
    let mut skipped_translucent_primitives = 0u64;
    let mut cache_misses_before = 0u64;
    let mut cache_misses_after = 0u64;
    let mut per_primitive = Vec::new();

    for (mesh_index, mesh) in meshes.iter().enumerate() {
        for (primitive_index, primitive) in array(mesh, "primitives").iter().enumerate() {
            let mode = primitive.get("mode").and_then(Value::as_u64).unwrap_or(4);
            if mode != 4 {
                return Err(CoreError::OperationUnavailable(format!(
                    "Mesh {mesh_index} primitive {primitive_index} uses primitive mode {mode}; render optimization supports triangles only."
                )));
            }
            let indices = read_primitive_indices(&glb.document, binary, primitive)?;
            if !indices.len().is_multiple_of(3) {
                return Err(CoreError::InvalidInput(
                    "Triangle index count is not divisible by three.".into(),
                ));
            }
            triangles = triangles
                .checked_add((indices.len() / 3) as u64)
                .ok_or_else(|| CoreError::InvalidInput("Triangle count overflowed.".into()))?;
            let misses_before = simulate_vertex_cache_misses(&indices, CACHE_SIZE);
            cache_misses_before += misses_before;
            if primitive_uses_blending(&glb.document, primitive)? {
                skipped_translucent_primitives += 1;
                cache_misses_after += misses_before;
                continue;
            }

            let positions = read_cleanup_positions(&glb.document, binary, primitive)?;
            if indices
                .iter()
                .any(|index| *index as usize >= positions.len())
            {
                return Err(CoreError::InvalidInput(
                    "Primitive index refers to a vertex outside POSITION.".into(),
                ));
            }
            let mut position_bytes = Vec::with_capacity(positions.len() * 12);
            for position in &positions {
                for component in position {
                    position_bytes.extend_from_slice(&component.to_le_bytes());
                }
            }
            let adapter = VertexDataAdapter::new(&position_bytes, 12, 0).map_err(|error| {
                CoreError::InvalidInput(format!("Invalid POSITION data: {error}"))
            })?;
            let mut optimized = meshopt::optimize_vertex_cache(&indices, positions.len());
            meshopt::optimize_overdraw_in_place(&mut optimized, &adapter, overdraw_threshold);
            let misses_after = simulate_vertex_cache_misses(&optimized, CACHE_SIZE);
            cache_misses_after += misses_after;
            processed_primitives += 1;
            if optimized != indices {
                reordered_primitives += 1;
                per_primitive.push((mesh_index, primitive_index, optimized));
            }
        }
    }
    if triangles == 0 {
        return Err(CoreError::InvalidInput(
            "GLB contains no indexed triangle primitives to optimize.".into(),
        ));
    }

    for (mesh_index, primitive_index, indices) in per_primitive {
        let accessor_index = append_index_accessor(&mut glb.document, binary, &indices)?;
        glb.document["meshes"][mesh_index]["primitives"][primitive_index]["indices"] =
            json!(accessor_index);
    }
    let encoded = if reordered_primitives == 0 {
        bytes.to_vec()
    } else {
        let buffers = glb
            .document
            .get_mut("buffers")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| CoreError::InvalidInput("GLB buffer metadata is missing.".into()))?;
        buffers[0]["byteLength"] = json!(binary.len());
        glb.write()?
    };
    let mut warnings = Vec::new();
    if skipped_translucent_primitives > 0 {
        warnings.push(format!(
            "Skipped {skipped_translucent_primitives} BLEND primitive(s) because triangle order affects alpha blending."
        ));
    }
    if cache_misses_after > cache_misses_before {
        warnings.push("The selected overdraw threshold traded vertex-cache efficiency for front-to-back order; inspect the report before using this asset in a vertex-bound workload.".into());
    }
    Ok((
        encoded,
        RenderOptimizationReport {
            triangles,
            processed_primitives,
            reordered_primitives,
            skipped_translucent_primitives,
            vertex_cache_size: CACHE_SIZE,
            vertex_cache_misses_before: cache_misses_before,
            vertex_cache_misses_after: cache_misses_after,
            overdraw_threshold,
            preserved: vec![
                "triangle count and vertex references".into(),
                "vertex attributes, materials, and textures".into(),
                "primitive and node hierarchy".into(),
                "translucent primitive order".into(),
            ],
            warnings,
        },
    ))
}

fn read_cleanup_positions(
    document: &Value,
    binary: &[u8],
    primitive: &Value,
) -> Result<Vec<[f32; 3]>, CoreError> {
    let position_index = primitive
        .get("attributes")
        .and_then(|attributes| attributes.get("POSITION"))
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("Triangle primitive is missing POSITION.".into()))?
        as usize;
    let accessor = array(document, "accessors")
        .get(position_index)
        .ok_or_else(|| CoreError::InvalidInput("POSITION accessor does not exist.".into()))?;
    if accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        || accessor
            .get("normalized")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Err(CoreError::OperationUnavailable(
            "POSITION must use unnormalized FLOAT components.".into(),
        ));
    }
    decode_float_vectors(document, binary, position_index, "VEC3", "POSITION")?
        .into_iter()
        .map(|position| {
            let result = [position[0], position[1], position[2]];
            if result.iter().any(|component| !component.is_finite()) {
                return Err(CoreError::InvalidInput(
                    "POSITION contains a non-finite value.".into(),
                ));
            }
            Ok(result)
        })
        .collect()
}

fn triangle_has_zero_area(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> bool {
    let ab = [
        f64::from(b[0]) - f64::from(a[0]),
        f64::from(b[1]) - f64::from(a[1]),
        f64::from(b[2]) - f64::from(a[2]),
    ];
    let ac = [
        f64::from(c[0]) - f64::from(a[0]),
        f64::from(c[1]) - f64::from(a[1]),
        f64::from(c[2]) - f64::from(a[2]),
    ];
    let cross = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    cross.iter().all(|component| *component == 0.0)
}

fn canonical_oriented_triangle(a: u32, b: u32, c: u32) -> [u32; 3] {
    if a <= b && a <= c {
        [a, b, c]
    } else if b <= a && b <= c {
        [b, c, a]
    } else {
        [c, a, b]
    }
}

fn primitive_uses_blending(document: &Value, primitive: &Value) -> Result<bool, CoreError> {
    let Some(material_index) = primitive.get("material").and_then(Value::as_u64) else {
        return Ok(false);
    };
    let material = array(document, "materials")
        .get(material_index as usize)
        .ok_or_else(|| {
            CoreError::InvalidInput("Primitive references a missing material.".into())
        })?;
    Ok(material
        .get("alphaMode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode == "BLEND"))
}

fn simulate_vertex_cache_misses(indices: &[u32], cache_size: usize) -> u64 {
    let mut cache = VecDeque::with_capacity(cache_size);
    let mut misses = 0u64;
    for index in indices {
        if let Some(position) = cache.iter().position(|cached| cached == index) {
            let _ = cache.remove(position);
            cache.push_front(*index);
        } else {
            misses += 1;
            cache.push_front(*index);
            if cache.len() > cache_size {
                cache.pop_back();
            }
        }
    }
    misses
}

/// Simplifies triangle primitives in a GLB with meshoptimizer and returns an
/// independently encoded derived GLB plus an auditable summary. Positions,
/// normals, UVs, materials, textures, and the rest of each vertex stream stay
/// byte-for-byte unchanged; only primitive index references are replaced.
pub fn simplify_glb(
    bytes: &[u8],
    target_triangles: u64,
    max_error: f32,
) -> Result<(Vec<u8>, SimplificationReport), CoreError> {
    if target_triangles == 0 {
        return Err(CoreError::InvalidInput(
            "target_triangles must be greater than zero.".into(),
        ));
    }
    if !max_error.is_finite() || !(0.0..=1.0).contains(&max_error) {
        return Err(CoreError::InvalidInput(
            "max_error must be a finite value from 0 to 1.".into(),
        ));
    }

    let mut glb = GlbFile::parse(bytes)?;
    validate_static_glb(&glb.document)?;
    let binary = glb
        .binary
        .as_mut()
        .ok_or_else(|| CoreError::InvalidInput("GLB has no embedded BIN chunk.".into()))?;
    validate_single_embedded_buffer(&glb.document, binary.len())?;

    let meshes = glb
        .document
        .get("meshes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut primitives = Vec::new();
    let mut total_triangles = 0u64;
    for (mesh_index, mesh) in meshes.iter().enumerate() {
        for (primitive_index, primitive) in array(mesh, "primitives").iter().enumerate() {
            let mode = primitive.get("mode").and_then(Value::as_u64).unwrap_or(4);
            if mode != 4 {
                return Err(CoreError::OperationUnavailable(format!(
                    "Mesh {mesh_index} primitive {primitive_index} uses primitive mode {mode}; only triangles are supported by simplification."
                )));
            }
            let indices = read_primitive_indices(&glb.document, binary, primitive)?;
            let count = (indices.len() / 3) as u64;
            if count == 0 {
                return Err(CoreError::InvalidInput(
                    "Cannot simplify an empty triangle primitive.".into(),
                ));
            }
            total_triangles = total_triangles
                .checked_add(count)
                .ok_or_else(|| CoreError::InvalidInput("Triangle count overflowed.".into()))?;
            primitives.push((mesh_index, primitive_index, count));
        }
    }
    if primitives.is_empty() || total_triangles == 0 {
        return Err(CoreError::InvalidInput(
            "GLB contains no indexed triangle primitives to simplify.".into(),
        ));
    }
    if target_triangles >= total_triangles {
        return Err(CoreError::InvalidInput(format!(
            "target_triangles ({target_triangles}) must be lower than the input triangle count ({total_triangles})."
        )));
    }

    let mut report_error = 0.0f32;
    let mut actual_triangles = 0u64;
    let mut optimized_primitives = 0u64;
    let mut per_primitive = Vec::with_capacity(primitives.len());
    for (mesh_index, primitive_index, original_count) in &primitives {
        let target_for_primitive = ((*original_count as u128 * target_triangles as u128)
            / total_triangles as u128)
            .max(1)
            .min(*original_count as u128) as u64;
        let indices = {
            let primitive = &glb.document["meshes"][*mesh_index]["primitives"][*primitive_index];
            read_primitive_indices(&glb.document, binary, primitive)?
        };
        let primitive = &glb.document["meshes"][*mesh_index]["primitives"][*primitive_index];
        let position_index = primitive
            .get("attributes")
            .and_then(|a| a.get("POSITION"))
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                CoreError::InvalidInput("Triangle primitive is missing POSITION.".into())
            })? as usize;
        let position_accessor = array(&glb.document, "accessors")
            .get(position_index)
            .ok_or_else(|| CoreError::InvalidInput("POSITION accessor does not exist.".into()))?;
        if position_accessor
            .get("componentType")
            .and_then(Value::as_u64)
            != Some(5126)
            || position_accessor
                .get("normalized")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return Err(CoreError::OperationUnavailable(
                "POSITION must use unnormalized FLOAT components.".into(),
            ));
        }
        let positions =
            decode_float_vectors(&glb.document, binary, position_index, "VEC3", "POSITION")?;
        if indices
            .iter()
            .any(|index| *index as usize >= positions.len())
        {
            return Err(CoreError::InvalidInput(
                "Primitive index refers to a vertex outside POSITION.".into(),
            ));
        }
        let mut position_bytes = Vec::with_capacity(positions.len() * 12);
        for position in &positions {
            for component in position {
                if !component.is_finite() {
                    return Err(CoreError::InvalidInput(
                        "POSITION contains a non-finite value.".into(),
                    ));
                }
                position_bytes.extend_from_slice(&component.to_le_bytes());
            }
        }
        let adapter = VertexDataAdapter::new(&position_bytes, 12, 0)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid POSITION data: {e}")))?;
        let (attributes, weights, attribute_stride) =
            decode_simplification_attributes(&glb.document, binary, primitive, positions.len())?;
        let mut error = 0.0f32;
        let locks = vec![SimplifyVertex::None; positions.len()];
        let reduced = if attributes.is_empty() {
            meshopt::simplify(
                &indices,
                &adapter,
                target_for_primitive as usize * 3,
                max_error,
                SimplifyOptions::LockBorder,
                Some(&mut error),
            )
        } else {
            meshopt::simplify_with_attributes_and_locks(
                &indices,
                &adapter,
                &attributes,
                &weights,
                attribute_stride,
                &locks,
                target_for_primitive as usize * 3,
                max_error,
                SimplifyOptions::LockBorder,
                Some(&mut error),
            )
        };
        if reduced.is_empty() || reduced.len() % 3 != 0 {
            return Err(CoreError::OperationUnavailable("meshoptimizer could not produce a valid triangle index buffer under the requested error limit.".into()));
        }
        let actual = (reduced.len() / 3) as u64;
        actual_triangles = actual_triangles.checked_add(actual).ok_or_else(|| {
            CoreError::InvalidInput("Simplified triangle count overflowed.".into())
        })?;
        optimized_primitives += 1;
        report_error = report_error.max(error);
        per_primitive.push((*mesh_index, *primitive_index, reduced));
    }

    for (mesh_index, primitive_index, indices) in per_primitive {
        let accessor_index = append_index_accessor(&mut glb.document, binary, &indices)?;
        glb.document["meshes"][mesh_index]["primitives"][primitive_index]["indices"] =
            json!(accessor_index);
    }
    let buffers = glb
        .document
        .get_mut("buffers")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CoreError::InvalidInput("GLB buffer metadata is missing.".into()))?;
    buffers[0]["byteLength"] = json!(binary.len());
    let encoded = glb.write()?;
    Ok((
        encoded,
        SimplificationReport {
            triangles_before: total_triangles,
            triangles_after: actual_triangles,
            target_triangles,
            geometric_error: report_error,
            optimized_primitives,
            preserved: vec![
                "vertex positions".into(),
                "vertex normals (where supported)".into(),
                "UV coordinates (where supported)".into(),
                "materials and textures".into(),
                "primitive and node hierarchy".into(),
            ],
            warnings: if actual_triangles > target_triangles {
                vec![format!(
                    "The requested target of {target_triangles} triangles was not reached under the configured error and border constraints; output contains {actual_triangles} triangles."
                )]
            } else {
                Vec::new()
            },
        },
    ))
}

const MAX_RETOPOLOGY_INPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_RETOPOLOGY_OUTPUT_BYTES: usize = 128 * 1024 * 1024;

pub(crate) fn extract_retopology_geometry(
    bytes: &[u8],
) -> Result<(Vec<[f32; 3]>, Vec<u32>), CoreError> {
    if bytes.len() > MAX_RETOPOLOGY_INPUT_BYTES {
        return Err(CoreError::OperationUnavailable(
            "Retopology accepts GLB files up to 64 MiB.".into(),
        ));
    }

    let glb = GlbFile::parse(bytes)?;
    let binary = glb.binary.as_ref().ok_or_else(|| {
        CoreError::OperationUnavailable("Retopology requires an embedded GLB buffer.".into())
    })?;
    validate_single_embedded_buffer(&glb.document, binary.len())?;
    validate_retopology_glb_document(&glb.document)?;

    let primitive = &glb.document["meshes"][0]["primitives"][0];
    let attributes = primitive
        .get("attributes")
        .and_then(Value::as_object)
        .ok_or_else(|| CoreError::InvalidInput("Primitive attributes are missing.".into()))?;
    let position_accessor_index = attributes
        .get("POSITION")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("POSITION accessor is missing.".into()))?
        as usize;
    let accessors = array(&glb.document, "accessors");
    let position_accessor = accessors
        .get(position_accessor_index)
        .ok_or_else(|| CoreError::InvalidInput("POSITION references a missing accessor.".into()))?;
    if position_accessor
        .get("componentType")
        .and_then(Value::as_u64)
        != Some(5126)
        || position_accessor.get("normalized").and_then(Value::as_bool) == Some(true)
        || position_accessor.get("type").and_then(Value::as_str) != Some("VEC3")
        || position_accessor.get("sparse").is_some()
    {
        return Err(CoreError::OperationUnavailable(
            "Retopology requires a non-sparse float32 VEC3 POSITION accessor.".into(),
        ));
    }
    let position_count = position_accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("POSITION count is missing.".into()))?
        as usize;
    if position_count == 0 || position_count > 100_000 {
        return Err(CoreError::OperationUnavailable(
            "Retopology accepts 1 to 100,000 POSITION vertices.".into(),
        ));
    }

    let index_accessor_index = primitive
        .get("indices")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CoreError::OperationUnavailable("Retopology requires indexed triangles.".into())
        })? as usize;
    let index_accessor = accessors.get(index_accessor_index).ok_or_else(|| {
        CoreError::InvalidInput("Primitive references a missing index accessor.".into())
    })?;
    let index_count = index_accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("Index count is missing.".into()))?
        as usize;
    if index_count == 0 || !index_count.is_multiple_of(3) || index_count > 600_000 {
        return Err(CoreError::OperationUnavailable(
            "Retopology accepts 1 to 200,000 indexed triangles.".into(),
        ));
    }
    if index_accessor.get("normalized").and_then(Value::as_bool) == Some(true) {
        return Err(CoreError::OperationUnavailable(
            "Normalized index accessors are not supported.".into(),
        ));
    }

    let position_vectors = decode_float_vectors(
        &glb.document,
        binary,
        position_accessor_index,
        "VEC3",
        "POSITION",
    )?;
    let positions = position_vectors
        .into_iter()
        .map(|position| [position[0], position[1], position[2]])
        .collect();
    let indices = read_primitive_indices(&glb.document, binary, primitive)?;
    Ok((positions, indices))
}

pub fn retopology_glb(
    bytes: &[u8],
    options: RetopologyOptions,
) -> Result<(Vec<u8>, RetopologyReport), CoreError> {
    if bytes.len() > MAX_RETOPOLOGY_INPUT_BYTES {
        return Err(CoreError::OperationUnavailable(
            "Retopology accepts GLB files up to 64 MiB.".into(),
        ));
    }

    let mut glb = GlbFile::parse(bytes)?;
    let mut binary = glb.binary.take().ok_or_else(|| {
        CoreError::OperationUnavailable("Retopology requires an embedded GLB buffer.".into())
    })?;
    validate_single_embedded_buffer(&glb.document, binary.len())?;
    validate_retopology_glb_document(&glb.document)?;

    let primitive = &glb.document["meshes"][0]["primitives"][0];
    let attributes = primitive
        .get("attributes")
        .and_then(Value::as_object)
        .ok_or_else(|| CoreError::InvalidInput("Primitive attributes are missing.".into()))?;
    let position_accessor_index = attributes
        .get("POSITION")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("POSITION accessor is missing.".into()))?
        as usize;
    let accessors = array(&glb.document, "accessors");
    let position_accessor = accessors
        .get(position_accessor_index)
        .ok_or_else(|| CoreError::InvalidInput("POSITION references a missing accessor.".into()))?;
    if position_accessor
        .get("componentType")
        .and_then(Value::as_u64)
        != Some(5126)
        || position_accessor.get("normalized").and_then(Value::as_bool) == Some(true)
        || position_accessor.get("type").and_then(Value::as_str) != Some("VEC3")
        || position_accessor.get("sparse").is_some()
    {
        return Err(CoreError::OperationUnavailable(
            "Retopology requires a non-sparse float32 VEC3 POSITION accessor.".into(),
        ));
    }
    let position_count = position_accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("POSITION count is missing.".into()))?
        as usize;
    if position_count == 0 || position_count > 100_000 {
        return Err(CoreError::OperationUnavailable(
            "Retopology accepts 1 to 100,000 POSITION vertices.".into(),
        ));
    }

    let index_accessor_index = primitive
        .get("indices")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CoreError::OperationUnavailable("Retopology requires indexed triangles.".into())
        })? as usize;
    let index_accessor = accessors.get(index_accessor_index).ok_or_else(|| {
        CoreError::InvalidInput("Primitive references a missing index accessor.".into())
    })?;
    let index_count = index_accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("Index count is missing.".into()))?
        as usize;
    if index_count == 0 || !index_count.is_multiple_of(3) || index_count > 600_000 {
        return Err(CoreError::OperationUnavailable(
            "Retopology accepts 1 to 200,000 indexed triangles.".into(),
        ));
    }
    if index_accessor.get("normalized").and_then(Value::as_bool) == Some(true) {
        return Err(CoreError::OperationUnavailable(
            "Normalized index accessors are not supported.".into(),
        ));
    }

    let position_vectors = decode_float_vectors(
        &glb.document,
        &binary,
        position_accessor_index,
        "VEC3",
        "POSITION",
    )?;
    let mut positions = Vec::with_capacity(position_vectors.len());
    for position in position_vectors {
        positions.push([position[0], position[1], position[2]]);
    }
    let input_indices = read_primitive_indices(&glb.document, &binary, primitive)?;
    let (response, mut report) = remesh_triangles_in_worker(&positions, &input_indices, options)?;

    let normals = generate_vertex_normals(&response.positions, &response.indices)?;
    let new_position_accessor =
        append_float_vec3_accessor(&mut glb.document, &mut binary, &response.positions, true)?;
    let new_normal_accessor =
        append_float_vec3_accessor(&mut glb.document, &mut binary, &normals, false)?;
    let new_index_accessor =
        append_index_accessor(&mut glb.document, &mut binary, &response.indices)?;

    let primitive = &mut glb.document["meshes"][0]["primitives"][0];
    primitive["attributes"] = json!({
        "POSITION": new_position_accessor,
        "NORMAL": new_normal_accessor,
    });
    primitive["indices"] = json!(new_index_accessor);
    primitive["mode"] = json!(4);
    glb.document["buffers"][0]["byteLength"] = json!(binary.len());
    glb.binary = Some(binary);

    report.warnings.push(
        "GLB mode is limited to one closed, untextured static triangle mesh; vertex normals are recomputed, and source textures/UVs are not transferred.".into(),
    );
    let output = glb.write()?;
    if output.len() > MAX_RETOPOLOGY_OUTPUT_BYTES {
        return Err(CoreError::OperationUnavailable(
            "Retopology output exceeds the 128 MiB GLB limit.".into(),
        ));
    }
    Ok((output, report))
}

fn validate_retopology_glb_document(document: &Value) -> Result<(), CoreError> {
    if !array(document, "animations").is_empty() || !array(document, "skins").is_empty() {
        return Err(CoreError::OperationUnavailable(
            "Retopology does not support animated or skinned GLB assets.".into(),
        ));
    }
    if !array(document, "extensionsUsed").is_empty()
        || !array(document, "extensionsRequired").is_empty()
        || has_nonempty_extensions(document)
    {
        return Err(CoreError::OperationUnavailable(
            "Retopology does not support GLB extensions.".into(),
        ));
    }
    if !array(document, "textures").is_empty()
        || !array(document, "images").is_empty()
        || !array(document, "samplers").is_empty()
    {
        return Err(CoreError::OperationUnavailable(
            "Retopology is geometry-only and does not support textures or images.".into(),
        ));
    }

    let meshes = array(document, "meshes");
    if meshes.len() != 1 || array(&meshes[0], "primitives").len() != 1 {
        return Err(CoreError::OperationUnavailable(
            "Retopology requires exactly one mesh with one primitive.".into(),
        ));
    }
    if meshes[0].get("weights").is_some() {
        return Err(CoreError::OperationUnavailable(
            "Retopology does not support mesh morph weights.".into(),
        ));
    }
    let nodes = array(document, "nodes");
    let scenes = array(document, "scenes");
    if nodes.len() != 1
        || nodes[0].get("mesh").and_then(Value::as_u64) != Some(0)
        || nodes[0].get("skin").is_some()
        || nodes[0].get("camera").is_some()
        || !array(&nodes[0], "children").is_empty()
        || scenes.len() != 1
        || array(&scenes[0], "nodes").len() != 1
        || array(&scenes[0], "nodes")[0].as_u64() != Some(0)
        || document.get("scene").and_then(Value::as_u64).unwrap_or(0) != 0
        || !array(document, "cameras").is_empty()
    {
        return Err(CoreError::OperationUnavailable(
            "Retopology requires one scene containing a single node that references mesh 0.".into(),
        ));
    }

    let primitive = &meshes[0]["primitives"][0];
    if primitive.get("mode").and_then(Value::as_u64).unwrap_or(4) != 4
        || primitive.get("targets").is_some()
    {
        return Err(CoreError::OperationUnavailable(
            "Retopology supports static TRIANGLES primitives without morph targets.".into(),
        ));
    }
    let attributes = primitive
        .get("attributes")
        .and_then(Value::as_object)
        .ok_or_else(|| CoreError::InvalidInput("Primitive attributes are missing.".into()))?;
    if attributes.len() != 1 || !attributes.contains_key("POSITION") {
        return Err(CoreError::OperationUnavailable(
            "Retopology accepts POSITION only; normals, UVs, colors, skinning, and custom attributes are not transferable.".into(),
        ));
    }

    let materials = array(document, "materials");
    if materials.len() > 1
        || primitive
            .get("material")
            .and_then(Value::as_u64)
            .is_some_and(|index| index as usize >= materials.len())
    {
        return Err(CoreError::OperationUnavailable(
            "Retopology accepts at most one supported untextured material.".into(),
        ));
    }
    if materials.iter().any(|material| {
        material.get("alphaMode").and_then(Value::as_str) == Some("BLEND")
            || contains_texture_reference(material)
    }) {
        return Err(CoreError::OperationUnavailable(
            "Retopology does not support textured or blended materials.".into(),
        ));
    }

    if array(document, "accessors").len() != 2 || array(document, "bufferViews").len() != 2 {
        return Err(CoreError::OperationUnavailable(
            "Retopology requires one POSITION accessor and one index accessor.".into(),
        ));
    }
    Ok(())
}

fn has_nonempty_extensions(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            (key == "extensions"
                && value
                    .as_object()
                    .is_some_and(|extensions| !extensions.is_empty()))
                || has_nonempty_extensions(value)
        }),
        Value::Array(values) => values.iter().any(has_nonempty_extensions),
        _ => false,
    }
}

fn contains_texture_reference(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            (key.ends_with("Texture") && !value.is_null()) || contains_texture_reference(value)
        }),
        Value::Array(values) => values.iter().any(contains_texture_reference),
        _ => false,
    }
}

fn generate_vertex_normals(
    positions: &[[f32; 3]],
    indices: &[u32],
) -> Result<Vec<[f32; 3]>, CoreError> {
    let mut normals = vec![[0.0f64; 3]; positions.len()];
    for triangle in indices.chunks_exact(3) {
        let [a, b, c] = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        let pa = positions[a].map(f64::from);
        let pb = positions[b].map(f64::from);
        let pc = positions[c].map(f64::from);
        let edge_a = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let edge_b = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let normal = [
            edge_a[1] * edge_b[2] - edge_a[2] * edge_b[1],
            edge_a[2] * edge_b[0] - edge_a[0] * edge_b[2],
            edge_a[0] * edge_b[1] - edge_a[1] * edge_b[0],
        ];
        for vertex in [a, b, c] {
            for axis in 0..3 {
                normals[vertex][axis] += normal[axis];
            }
        }
    }
    normals
        .into_iter()
        .map(|normal| {
            let length =
                (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
            if !length.is_finite() || length <= 1.0e-30 {
                return Err(CoreError::OperationUnavailable(
                    "Retopology could not recompute a vertex normal.".into(),
                ));
            }
            Ok([
                (normal[0] / length) as f32,
                (normal[1] / length) as f32,
                (normal[2] / length) as f32,
            ])
        })
        .collect()
}

fn append_float_vec3_accessor(
    document: &mut Value,
    binary: &mut Vec<u8>,
    values: &[[f32; 3]],
    include_bounds: bool,
) -> Result<usize, CoreError> {
    pad_vec(binary, 0);
    let byte_offset = binary.len();
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for value in values {
        if value.iter().any(|component| !component.is_finite()) {
            return Err(CoreError::OperationUnavailable(
                "Retopology produced non-finite vertex data.".into(),
            ));
        }
        for axis in 0..3 {
            min[axis] = min[axis].min(value[axis]);
            max[axis] = max[axis].max(value[axis]);
            binary.extend_from_slice(&value[axis].to_le_bytes());
        }
    }
    let byte_length = values
        .len()
        .checked_mul(12)
        .ok_or_else(|| CoreError::InvalidInput("Vertex buffer length overflowed.".into()))?;
    let views = document
        .get_mut("bufferViews")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CoreError::InvalidInput("GLB bufferViews are missing.".into()))?;
    let view_index = views.len();
    views.push(json!({
        "buffer": 0,
        "byteOffset": byte_offset,
        "byteLength": byte_length,
        "byteStride": 12,
        "target": 34962
    }));
    let accessors = document
        .get_mut("accessors")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CoreError::InvalidInput("GLB accessors are missing.".into()))?;
    let accessor_index = accessors.len();
    let mut accessor = json!({
        "bufferView": view_index,
        "byteOffset": 0,
        "componentType": 5126,
        "count": values.len(),
        "type": "VEC3"
    });
    if include_bounds {
        accessor["min"] = json!(min);
        accessor["max"] = json!(max);
    }
    accessors.push(accessor);
    Ok(accessor_index)
}

fn validate_static_glb(document: &Value) -> Result<(), CoreError> {
    if !array(document, "animations").is_empty() {
        return Err(CoreError::OperationUnavailable(
            "Animated GLB assets are not supported by simplification.".into(),
        ));
    }
    if !array(document, "skins").is_empty()
        || array(document, "nodes")
            .iter()
            .any(|node| node.get("skin").is_some())
    {
        return Err(CoreError::OperationUnavailable(
            "Skinned GLB assets are not supported by simplification.".into(),
        ));
    }
    for (mesh_index, mesh) in array(document, "meshes").iter().enumerate() {
        for (primitive_index, primitive) in array(mesh, "primitives").iter().enumerate() {
            if !array(primitive, "targets").is_empty() {
                return Err(CoreError::OperationUnavailable(format!(
                    "Mesh {mesh_index} primitive {primitive_index} has morph targets, which are not supported."
                )));
            }
            if primitive.get("extensions").is_some() {
                return Err(CoreError::OperationUnavailable(format!(
                    "Mesh {mesh_index} primitive {primitive_index} uses an extension that may encode vertex or index data; extension-bearing primitives are not supported."
                )));
            }
        }
    }
    if array(document, "extensionsUsed").iter().any(|extension| {
        extension
            .as_str()
            .is_some_and(|s| s.contains("meshopt") || s.contains("draco"))
    }) {
        return Err(CoreError::OperationUnavailable(
            "Compressed mesh extensions are not supported by simplification.".into(),
        ));
    }
    Ok(())
}

fn validate_single_embedded_buffer(document: &Value, binary_len: usize) -> Result<(), CoreError> {
    let buffers = array(document, "buffers");
    if buffers.len() != 1 || buffers[0].get("uri").is_some() {
        return Err(CoreError::OperationUnavailable(
            "Simplification supports one embedded GLB buffer and does not resolve external URIs."
                .into(),
        ));
    }
    let declared = buffers[0]
        .get("byteLength")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CoreError::InvalidInput("Embedded buffer byteLength is missing or invalid.".into())
        })? as usize;
    if declared > binary_len {
        return Err(CoreError::InvalidInput(
            "Embedded buffer is shorter than its declared byteLength.".into(),
        ));
    }
    Ok(())
}

fn read_primitive_indices(
    document: &Value,
    binary: &[u8],
    primitive: &Value,
) -> Result<Vec<u32>, CoreError> {
    let accessor_index = primitive
        .get("indices")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CoreError::OperationUnavailable(
                "Simplification requires indexed triangle primitives.".into(),
            )
        })? as usize;
    let accessor = array(document, "accessors")
        .get(accessor_index)
        .ok_or_else(|| {
            CoreError::InvalidInput("Primitive references a missing index accessor.".into())
        })?;
    if accessor.get("type").and_then(Value::as_str) != Some("SCALAR")
        || accessor.get("sparse").is_some()
    {
        return Err(CoreError::OperationUnavailable(
            "Index accessor must be a non-sparse SCALAR.".into(),
        ));
    }
    let view_index = accessor
        .get("bufferView")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CoreError::OperationUnavailable("Index accessor has no bufferView.".into())
        })? as usize;
    let view = array(document, "bufferViews")
        .get(view_index)
        .ok_or_else(|| {
            CoreError::InvalidInput("Index accessor references a missing bufferView.".into())
        })?;
    if view.get("buffer").and_then(Value::as_u64).unwrap_or(0) != 0 {
        return Err(CoreError::OperationUnavailable(
            "Index accessor references a non-GLB buffer.".into(),
        ));
    }
    let component_type = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CoreError::InvalidInput("Index accessor componentType is missing.".into())
        })?;
    let element_size = match component_type {
        5121 => 1usize,
        5123 => 2,
        5125 => 4,
        _ => {
            return Err(CoreError::OperationUnavailable(
                "Index components must be unsigned byte, unsigned short, or unsigned int.".into(),
            ));
        }
    };
    let count = accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("Index accessor count is missing.".into()))?
        as usize;
    if count == 0 || !count.is_multiple_of(3) {
        return Err(CoreError::InvalidInput(
            "Triangle index count must be nonzero and divisible by three.".into(),
        ));
    }
    let view_offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let view_length = view
        .get("byteLength")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput("Index bufferView byteLength is missing.".into()))?
        as usize;
    let view_end = view_offset
        .checked_add(view_length)
        .filter(|end| *end <= binary.len())
        .ok_or_else(|| {
            CoreError::InvalidInput("Index bufferView exceeds the GLB binary chunk.".into())
        })?;
    let accessor_offset = accessor
        .get("byteOffset")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let stride = view
        .get("byteStride")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(element_size);
    if stride < element_size || !stride.is_multiple_of(element_size) {
        return Err(CoreError::InvalidInput(
            "Index byteStride is invalid.".into(),
        ));
    }
    let start = view_offset
        .checked_add(accessor_offset)
        .ok_or_else(|| CoreError::InvalidInput("Index offset overflowed.".into()))?;
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let offset = start
            .checked_add(
                index
                    .checked_mul(stride)
                    .ok_or_else(|| CoreError::InvalidInput("Index offset overflowed.".into()))?,
            )
            .ok_or_else(|| CoreError::InvalidInput("Index offset overflowed.".into()))?;
        let element_end = offset
            .checked_add(element_size)
            .filter(|end| *end <= view_end)
            .ok_or_else(|| {
                CoreError::InvalidInput("Index accessor exceeds the GLB binary chunk.".into())
            })?;
        let slice = binary.get(offset..element_end).ok_or_else(|| {
            CoreError::InvalidInput("Index accessor exceeds the GLB binary chunk.".into())
        })?;
        result.push(match component_type {
            5121 => slice[0] as u32,
            5123 => u16::from_le_bytes([slice[0], slice[1]]) as u32,
            5125 => u32::from_le_bytes(slice.try_into().expect("4-byte index")),
            _ => unreachable!(),
        });
    }
    Ok(result)
}

fn decode_float_vectors(
    document: &Value,
    binary: &[u8],
    accessor_index: usize,
    expected_type: &str,
    semantic: &str,
) -> Result<Vec<Vec<f32>>, CoreError> {
    let accessor = array(document, "accessors")
        .get(accessor_index)
        .ok_or_else(|| CoreError::InvalidInput(format!("{semantic} accessor does not exist.")))?;
    if accessor.get("type").and_then(Value::as_str) != Some(expected_type)
        || accessor.get("sparse").is_some()
    {
        return Err(CoreError::OperationUnavailable(format!(
            "{semantic} accessor must be a non-sparse {expected_type}."
        )));
    }
    let component_type = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput(format!("{semantic} componentType is missing.")))?;
    let component_size = component_size(component_type)?;
    let dimensions = match expected_type {
        "VEC2" => 2usize,
        "VEC3" => 3,
        "VEC4" => 4,
        _ => {
            return Err(CoreError::InvalidInput(
                "Unsupported vector accessor type.".into(),
            ));
        }
    };
    if component_type == 5125 {
        return Err(CoreError::OperationUnavailable(format!(
            "{semantic} cannot use unsigned-int components."
        )));
    }
    let view_index = accessor
        .get("bufferView")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CoreError::OperationUnavailable(format!("{semantic} accessor has no bufferView."))
        })? as usize;
    let view = array(document, "bufferViews")
        .get(view_index)
        .ok_or_else(|| {
            CoreError::InvalidInput(format!("{semantic} references a missing bufferView."))
        })?;
    if view.get("buffer").and_then(Value::as_u64).unwrap_or(0) != 0 {
        return Err(CoreError::OperationUnavailable(format!(
            "{semantic} references a non-GLB buffer."
        )));
    }
    let count = accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::InvalidInput(format!("{semantic} count is missing.")))?
        as usize;
    let element_size = component_size * dimensions;
    let stride = view
        .get("byteStride")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(element_size);
    if stride < element_size || !stride.is_multiple_of(component_size) {
        return Err(CoreError::InvalidInput(format!(
            "{semantic} byteStride is invalid."
        )));
    }
    let view_offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let view_length = view
        .get("byteLength")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CoreError::InvalidInput(format!("{semantic} bufferView byteLength is missing."))
        })? as usize;
    let view_end = view_offset
        .checked_add(view_length)
        .filter(|end| *end <= binary.len())
        .ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "{semantic} bufferView exceeds the GLB binary chunk."
            ))
        })?;
    let start = view_offset
        .checked_add(
            accessor
                .get("byteOffset")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
        )
        .ok_or_else(|| CoreError::InvalidInput(format!("{semantic} byte offset overflowed.")))?;
    let normalized = accessor
        .get("normalized")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut vectors = Vec::with_capacity(count);
    for vertex in 0..count {
        let offset =
            start
                .checked_add(vertex.checked_mul(stride).ok_or_else(|| {
                    CoreError::InvalidInput(format!("{semantic} offset overflowed."))
                })?)
                .ok_or_else(|| CoreError::InvalidInput(format!("{semantic} offset overflowed.")))?;
        let element_end = offset
            .checked_add(element_size)
            .filter(|end| *end <= view_end)
            .ok_or_else(|| {
                CoreError::InvalidInput(format!(
                    "{semantic} accessor exceeds the GLB binary chunk."
                ))
            })?;
        let element = binary.get(offset..element_end).ok_or_else(|| {
            CoreError::InvalidInput(format!("{semantic} accessor exceeds the GLB binary chunk."))
        })?;
        let mut vector = Vec::with_capacity(dimensions);
        for component in 0..dimensions {
            let part = &element[component * component_size..(component + 1) * component_size];
            vector.push(decode_component(part, component_type, normalized)?);
        }
        vectors.push(vector);
    }
    Ok(vectors)
}

fn decode_simplification_attributes(
    document: &Value,
    binary: &[u8],
    primitive: &Value,
    vertex_count: usize,
) -> Result<(Vec<f32>, Vec<f32>, usize), CoreError> {
    let attributes = primitive
        .get("attributes")
        .and_then(Value::as_object)
        .ok_or_else(|| CoreError::InvalidInput("Primitive attributes are missing.".into()))?;
    let mut streams: Vec<(String, Vec<Vec<f32>>, f32)> = Vec::new();
    for (semantic, shape, weight) in [("NORMAL", "VEC3", 1.0f32), ("TEXCOORD_0", "VEC2", 25.0f32)] {
        let Some(index) = attributes.get(semantic).and_then(Value::as_u64) else {
            continue;
        };
        let values = decode_float_vectors(document, binary, index as usize, shape, semantic)?;
        if values.len() != vertex_count {
            return Err(CoreError::InvalidInput(format!(
                "{semantic} count does not match POSITION count."
            )));
        }
        streams.push((semantic.to_string(), values, weight));
    }
    let stride = streams
        .iter()
        .map(|(_, values, _)| values.first().map(Vec::len).unwrap_or(0))
        .sum::<usize>();
    if stride == 0 {
        return Ok((Vec::new(), Vec::new(), 0));
    }
    let mut flat = Vec::with_capacity(vertex_count * stride);
    let mut weights = Vec::with_capacity(stride);
    for (_, values, weight) in &streams {
        let components = values.first().map(Vec::len).unwrap_or(0);
        weights.extend(std::iter::repeat_n(*weight, components));
    }
    for vertex in 0..vertex_count {
        for (_, values, _) in &streams {
            flat.extend(values[vertex].iter().copied());
        }
    }
    Ok((flat, weights, stride))
}

fn decode_component(bytes: &[u8], component_type: u64, normalized: bool) -> Result<f32, CoreError> {
    let value = match component_type {
        5126 => f32::from_le_bytes(bytes.try_into().expect("4-byte float")),
        5120 => {
            let value = bytes[0] as i8;
            if normalized {
                (value as f32 / 127.0).max(-1.0)
            } else {
                value as f32
            }
        }
        5121 => {
            if normalized {
                bytes[0] as f32 / 255.0
            } else {
                bytes[0] as f32
            }
        }
        5122 => {
            let value = i16::from_le_bytes([bytes[0], bytes[1]]);
            if normalized {
                (value as f32 / 32767.0).max(-1.0)
            } else {
                value as f32
            }
        }
        5123 => {
            let value = u16::from_le_bytes([bytes[0], bytes[1]]);
            if normalized {
                value as f32 / 65535.0
            } else {
                value as f32
            }
        }
        _ => {
            return Err(CoreError::OperationUnavailable(format!(
                "Unsupported vertex attribute component type {component_type}."
            )));
        }
    };
    if !value.is_finite() {
        return Err(CoreError::InvalidInput(
            "Vertex attributes contain a non-finite value.".into(),
        ));
    }
    Ok(value)
}

fn component_size(component_type: u64) -> Result<usize, CoreError> {
    match component_type {
        5120 | 5121 => Ok(1),
        5122 | 5123 => Ok(2),
        5125 | 5126 => Ok(4),
        _ => Err(CoreError::OperationUnavailable(format!(
            "Unsupported accessor component type {component_type}."
        ))),
    }
}

fn append_index_accessor(
    document: &mut Value,
    binary: &mut Vec<u8>,
    indices: &[u32],
) -> Result<usize, CoreError> {
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    let byte_offset = binary.len();
    let minimum = indices.iter().min().copied().unwrap_or(0);
    let maximum = indices.iter().max().copied().unwrap_or(0);
    let (component_type, element_size) = if maximum <= u8::MAX as u32 {
        (5121u32, 1usize)
    } else if maximum <= u16::MAX as u32 {
        (5123u32, 2usize)
    } else {
        (5125u32, 4usize)
    };
    for index in indices {
        match element_size {
            1 => binary.push(*index as u8),
            2 => binary.extend_from_slice(&(*index as u16).to_le_bytes()),
            _ => binary.extend_from_slice(&index.to_le_bytes()),
        }
    }
    let byte_length = indices
        .len()
        .checked_mul(element_size)
        .ok_or_else(|| CoreError::InvalidInput("Index buffer length overflowed.".into()))?;
    let views = document
        .get_mut("bufferViews")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CoreError::InvalidInput("GLB bufferViews are missing.".into()))?;
    let view_index = views.len();
    views.push(json!({ "buffer": 0, "byteOffset": byte_offset, "byteLength": byte_length, "target": 34963 }));
    let accessors = document
        .get_mut("accessors")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CoreError::InvalidInput("GLB accessors are missing.".into()))?;
    let accessor_index = accessors.len();
    accessors.push(json!({ "bufferView": view_index, "byteOffset": 0, "componentType": component_type, "count": indices.len(), "type": "SCALAR", "min": [minimum], "max": [maximum] }));
    Ok(accessor_index)
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn pad_vec(bytes: &mut Vec<u8>, padding: u8) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(padding);
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, CoreError> {
    let end = offset
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| CoreError::InvalidInput("GLB header is truncated.".into()))?;
    Ok(u32::from_le_bytes(
        bytes[offset..end].try_into().expect("four-byte slice"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_glb(mut document: Value, mut binary: Vec<u8>) -> Vec<u8> {
        while !binary.len().is_multiple_of(4) {
            binary.push(0);
        }
        document["buffers"] = json!([{ "byteLength": binary.len() }]);
        let mut json_bytes = serde_json::to_vec(&document).unwrap();
        pad_vec(&mut json_bytes, b' ');
        let total_length = 12 + 8 + json_bytes.len() + 8 + binary.len();
        let mut glb = Vec::new();
        glb.extend_from_slice(GLB_MAGIC);
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total_length as u32).to_le_bytes());
        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(&JSON_CHUNK.to_le_bytes());
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        glb.extend_from_slice(&BIN_CHUNK.to_le_bytes());
        glb.extend_from_slice(&binary);
        glb
    }

    fn triangle_list_glb(
        positions: &[[f32; 3]],
        indices: &[u16],
        alpha_mode: Option<&str>,
    ) -> Vec<u8> {
        let mut binary = Vec::new();
        for position in positions {
            for component in position {
                binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        let index_offset = binary.len();
        for index in indices {
            binary.extend_from_slice(&index.to_le_bytes());
        }
        let mut document = json!({
            "asset": { "version": "2.0" },
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": positions.len() * 12, "byteStride": 12, "target": 34962 },
                { "buffer": 0, "byteOffset": index_offset, "byteLength": indices.len() * 2, "target": 34963 }
            ],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": positions.len(), "type": "VEC3" },
                { "bufferView": 1, "componentType": 5123, "count": indices.len(), "type": "SCALAR" }
            ],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1, "material": 0 }] }],
            "materials": [{ "pbrMetallicRoughness": { "baseColorFactor": [0.8, 0.5, 0.2, 1.0] } }],
            "nodes": [{ "mesh": 0 }],
            "scenes": [{ "nodes": [0] }],
            "scene": 0
        });
        if let Some(mode) = alpha_mode {
            document["materials"][0]["alphaMode"] = json!(mode);
        }
        pack_glb(document, binary)
    }

    fn tetrahedron_glb() -> Vec<u8> {
        let positions = [
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let indices = [0u16, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];
        let mut binary = Vec::new();
        for position in positions {
            for component in position {
                binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        let index_offset = binary.len();
        for index in indices {
            binary.extend_from_slice(&index.to_le_bytes());
        }
        pack_glb(
            json!({
                "asset": { "version": "2.0" },
                "bufferViews": [
                    { "buffer": 0, "byteOffset": 0, "byteLength": 48, "byteStride": 12, "target": 34962 },
                    { "buffer": 0, "byteOffset": index_offset, "byteLength": 24, "target": 34963 }
                ],
                "accessors": [
                    { "bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 1.0] },
                    { "bufferView": 1, "componentType": 5123, "count": 12, "type": "SCALAR", "min": [0], "max": [3] }
                ],
                "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1, "mode": 4 }] }],
                "nodes": [{ "mesh": 0 }],
                "scenes": [{ "nodes": [0] }],
                "scene": 0
            }),
            binary,
        )
    }

    fn grid_glb(side: usize) -> Vec<u8> {
        let mut binary = Vec::new();
        let mut indices = Vec::<u16>::new();
        for y in 0..side {
            for x in 0..side {
                for value in [x as f32, 0.0, y as f32] {
                    binary.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        let index_offset = binary.len();
        for y in 0..side - 1 {
            for x in 0..side - 1 {
                let a = (y * side + x) as u16;
                let b = a + 1;
                let c = a + side as u16;
                let d = c + 1;
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        for index in indices {
            binary.extend_from_slice(&index.to_le_bytes());
        }
        let position_bytes = side * side * 12;
        let index_count = (side - 1) * (side - 1) * 6;
        pack_glb(
            json!({
                "asset": { "version": "2.0" },
                "bufferViews": [
                    { "buffer": 0, "byteOffset": 0, "byteLength": position_bytes, "byteStride": 12, "target": 34962 },
                    { "buffer": 0, "byteOffset": index_offset, "byteLength": index_count * 2, "target": 34963 }
                ],
                "accessors": [
                    { "bufferView": 0, "componentType": 5126, "count": side * side, "type": "VEC3" },
                    { "bufferView": 1, "componentType": 5123, "count": index_count, "type": "SCALAR" }
                ],
                "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1, "material": 0 }] }],
                "materials": [{ "pbrMetallicRoughness": { "baseColorFactor": [0.8, 0.5, 0.2, 1.0] } }],
                "nodes": [{ "mesh": 0 }],
                "scenes": [{ "nodes": [0] }],
                "scene": 0
            }),
            binary,
        )
    }

    #[test]
    fn retopology_gate_accepts_a_minimal_untextured_tetrahedron() {
        let glb = GlbFile::parse(&tetrahedron_glb()).unwrap();
        validate_retopology_glb_document(&glb.document).unwrap();
        validate_single_embedded_buffer(&glb.document, glb.binary.as_ref().unwrap().len()).unwrap();
    }

    #[test]
    fn retopology_gate_rejects_uvs_textures_extensions_and_extra_primitives() {
        let glb = GlbFile::parse(&tetrahedron_glb()).unwrap();
        let mut document = glb.document.clone();
        document["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = json!(2);
        assert!(validate_retopology_glb_document(&document).is_err());

        let glb = GlbFile::parse(&tetrahedron_glb()).unwrap();
        let mut document = glb.document.clone();
        document["textures"] = json!([{}]);
        assert!(validate_retopology_glb_document(&document).is_err());

        let glb = GlbFile::parse(&tetrahedron_glb()).unwrap();
        let mut document = glb.document.clone();
        document["nodes"][0]["extensions"] = json!({ "VENDOR_example": {} });
        assert!(validate_retopology_glb_document(&document).is_err());

        let glb = GlbFile::parse(&tetrahedron_glb()).unwrap();
        let mut document = glb.document.clone();
        document["meshes"][0]["primitives"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "attributes": { "POSITION": 0 },
                "indices": 1,
                "mode": 4
            }));
        assert!(validate_retopology_glb_document(&document).is_err());
    }

    #[test]
    fn generated_vertex_normals_are_finite_and_unit_length() {
        let positions = [
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let indices = [0u32, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];
        let normals = generate_vertex_normals(&positions, &indices).unwrap();
        assert_eq!(normals.len(), positions.len());
        for normal in normals {
            assert!(normal.iter().all(|component| component.is_finite()));
            let length =
                (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
            assert!((length - 1.0).abs() < 1.0e-6);
        }
    }

    #[test]
    fn recognizes_images_by_signature() {
        assert_eq!(
            inspect_asset("photo.png", b"\x89PNG\r\n\x1a\nrest")
                .unwrap()
                .kind,
            AssetKind::Image
        );
        assert_eq!(
            inspect_asset("photo.jpg", &[0xff, 0xd8, 0xff, 0xe0])
                .unwrap()
                .mime_type,
            "image/jpeg"
        );
    }

    #[test]
    fn parses_glb_and_counts_triangle_indices() {
        let bytes = grid_glb(2);
        let descriptor = inspect_asset("grid.glb", &bytes).unwrap();
        assert_eq!(descriptor.kind, AssetKind::Mesh);
        assert_eq!(descriptor.metadata["triangles"], 2);
    }

    #[test]
    fn meshoptimizer_reduces_a_mesh_and_preserves_material_metadata() {
        let original = grid_glb(12);
        let (reduced, report) = simplify_glb(&original, 80, 1.0).unwrap();
        let descriptor = inspect_asset("reduced.glb", &reduced).unwrap();
        assert!(report.triangles_after < report.triangles_before);
        assert_eq!(descriptor.metadata["material_count"], 1);
        let parsed = GlbFile::parse(&reduced).unwrap();
        assert_eq!(parsed.document["meshes"][0]["primitives"][0]["material"], 0);
        assert_eq!(
            parsed.document["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"][0],
            0.8
        );
    }

    #[test]
    fn simplify_reports_target_that_cannot_be_reached() {
        let original = grid_glb(4);
        let error = simplify_glb(&original, 0, 0.1).unwrap_err();
        assert!(error.to_string().contains("greater than zero"));
    }

    #[test]
    fn rejects_bad_lengths_json_and_animation() {
        let mut bytes = grid_glb(2);
        bytes[8..12].copy_from_slice(&0u32.to_le_bytes());
        assert!(inspect_asset("bad.glb", &bytes).is_err());
        assert!(GlbFile::parse(b"glTF").is_err());
        let mut animated = GlbFile::parse(&grid_glb(2)).unwrap();
        animated.document["animations"] = json!([{}]);
        let animated_bytes = animated.write().unwrap();
        assert!(
            simplify_glb(&animated_bytes, 1, 0.1)
                .unwrap_err()
                .to_string()
                .contains("Animated")
        );
    }
    #[test]
    fn cleanup_removes_degenerate_and_same_winding_duplicate_faces_only() {
        let original = triangle_list_glb(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.5, 0.0, 0.0],
            ],
            &[
                0, 2, 1, // valid
                0, 2, 1, // duplicate same winding
                1, 2, 0, // opposite winding retained
                0, 1, 3, // exact zero-area
                0, 0, 2, // repeated index
            ],
            None,
        );
        let (cleaned, report) = cleanup_glb(&original).unwrap();
        assert_eq!(report.triangles_before, 5);
        assert_eq!(report.triangles_after, 2);
        assert_eq!(report.duplicate_triangles_removed, 1);
        assert_eq!(report.zero_area_triangles_removed, 1);
        assert_eq!(report.repeated_index_triangles_removed, 1);
        let glb = GlbFile::parse(&cleaned).unwrap();
        let indices = read_primitive_indices(
            &glb.document,
            glb.binary.as_ref().unwrap(),
            &glb.document["meshes"][0]["primitives"][0],
        )
        .unwrap();
        assert_eq!(indices.len(), 6);
    }

    #[test]
    fn render_optimization_preserves_triangle_count_and_translucent_order() {
        let opaque = grid_glb(12);
        let (optimized, report) = optimize_render_glb(&opaque, 1.05).unwrap();
        assert_eq!(report.triangles, 242);
        assert_eq!(report.processed_primitives, 1);
        assert_eq!(report.skipped_translucent_primitives, 0);
        assert_eq!(
            inspect_asset("optimized.glb", &optimized).unwrap().metadata["triangles"],
            242
        );

        let blended = triangle_list_glb(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            &[0, 1, 2],
            Some("BLEND"),
        );
        let (unchanged, blended_report) = optimize_render_glb(&blended, 1.05).unwrap();
        assert_eq!(unchanged, blended);
        assert_eq!(blended_report.processed_primitives, 0);
        assert_eq!(blended_report.skipped_translucent_primitives, 1);
    }

    #[test]
    fn meshopt_compression_is_required_and_reduces_a_static_glb() {
        let original = grid_glb(12);
        let (compressed, report) = compress_mesh_glb(&original).unwrap();
        assert!(compressed.len() < original.len());
        assert_eq!(report.extension, "KHR_meshopt_compression");
        assert!(report.extension_required);
        assert!(report.compressed_buffer_views > 0);
        let glb = GlbFile::parse(&compressed).unwrap();
        assert_eq!(glb.document["buffers"].as_array().unwrap().len(), 2);
        let buffers = array(&glb.document, "buffers");
        let fallback_buffer = &buffers[1];
        assert!(fallback_buffer.get("uri").is_none());
        assert_eq!(
            fallback_buffer["extensions"]["KHR_meshopt_compression"]["fallback"].as_bool(),
            Some(true)
        );
        let compressed_views = array(&glb.document, "bufferViews");
        let mut fallback_view_end = 0u64;
        for view in compressed_views {
            if view.get("buffer").and_then(Value::as_u64) == Some(1) {
                assert!(
                    view.get("extensions")
                        .and_then(|extensions| extensions.get("KHR_meshopt_compression"))
                        .is_some(),
                    "every fallback bufferView must have a compressed representation"
                );
                let end =
                    view["byteOffset"].as_u64().unwrap_or(0) + view["byteLength"].as_u64().unwrap();
                fallback_view_end = fallback_view_end.max(end);
            }
        }
        assert!(fallback_buffer["byteLength"].as_u64().unwrap() >= fallback_view_end);
        assert!(
            array(&glb.document, "extensionsRequired")
                .iter()
                .any(|extension| extension.as_str() == Some("KHR_meshopt_compression"))
        );
        let original_glb = GlbFile::parse(&original).unwrap();
        let original_binary = original_glb.binary.as_ref().unwrap();
        let original_indices = read_primitive_indices(
            &original_glb.document,
            original_binary,
            &original_glb.document["meshes"][0]["primitives"][0],
        )
        .unwrap();
        let compressed_views = array(&glb.document, "bufferViews")
            .iter()
            .enumerate()
            .filter_map(|(view_index, view)| {
                Some((
                    view_index,
                    view.get("extensions")?.get("KHR_meshopt_compression")?,
                ))
            })
            .collect::<Vec<_>>();
        assert!(!compressed_views.is_empty());
        for (view_index, extension) in compressed_views {
            let start = extension["byteOffset"].as_u64().unwrap() as usize;
            let length = extension["byteLength"].as_u64().unwrap() as usize;
            let compressed_data = &glb.binary.as_ref().unwrap()[start..start + length];
            let count = extension["count"].as_u64().unwrap() as usize;
            match extension["mode"].as_str().unwrap() {
                "ATTRIBUTES" => {
                    assert_eq!(extension["byteStride"].as_u64(), Some(12));
                    let decoded =
                        meshopt::decode_vertex_buffer::<[u8; 12]>(compressed_data, count).unwrap();
                    let (source_offset, source_length) = buffer_view_range(
                        &original_glb.document,
                        view_index,
                        original_binary.len(),
                    )
                    .unwrap();
                    let decoded_bytes = decoded
                        .iter()
                        .flat_map(|record| record.iter().copied())
                        .collect::<Vec<_>>();
                    assert_eq!(
                        decoded_bytes,
                        original_binary[source_offset..source_offset + source_length]
                    );
                }
                "TRIANGLES" => {
                    let decoded = match extension["byteStride"].as_u64().unwrap() {
                        2 => meshopt::decode_index_buffer::<u16>(compressed_data, count)
                            .unwrap()
                            .into_iter()
                            .map(u32::from)
                            .collect::<Vec<_>>(),
                        4 => meshopt::decode_index_buffer::<u32>(compressed_data, count).unwrap(),
                        stride => panic!("unexpected meshopt index stride {stride}"),
                    };
                    assert_eq!(decoded, original_indices);
                }
                mode => panic!("unexpected meshopt compression mode {mode}"),
            }
        }
    }

    #[test]
    fn compression_rejects_a_payload_too_small_to_save_space() {
        let original = grid_glb(2);
        assert!(compress_mesh_glb(&original).is_err());
    }
}
