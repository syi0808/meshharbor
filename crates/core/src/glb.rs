use meshopt::{SimplifyOptions, SimplifyVertex, VertexDataAdapter};
use serde_json::{Map, Value, json};

use crate::{AssetKind, CoreError};

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
    for index in indices {
        binary.extend_from_slice(&index.to_le_bytes());
    }
    let views = document
        .get_mut("bufferViews")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CoreError::InvalidInput("GLB bufferViews are missing.".into()))?;
    let view_index = views.len();
    views.push(json!({ "buffer": 0, "byteOffset": byte_offset, "byteLength": indices.len() * 4, "target": 34963 }));
    let accessors = document
        .get_mut("accessors")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CoreError::InvalidInput("GLB accessors are missing.".into()))?;
    let accessor_index = accessors.len();
    let minimum = indices.iter().min().copied().unwrap_or(0);
    let maximum = indices.iter().max().copied().unwrap_or(0);
    accessors.push(json!({ "bufferView": view_index, "byteOffset": 0, "componentType": 5125, "count": indices.len(), "type": "SCALAR", "min": [minimum], "max": [maximum] }));
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
}
