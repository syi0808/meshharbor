use std::collections::{BTreeMap, btree_map::Entry};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::CoreError;

const GLB_MAGIC: &[u8; 4] = b"glTF";
const JSON_CHUNK: u32 = 0x4e4f_534a;
const BIN_CHUNK: u32 = 0x004e_4942;
const MAX_PARTS: usize = 256;
const MAX_PART_GLB_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PART_JSON_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PART_VERTICES: u64 = 1_000_000;
const MAX_PART_INDICES: u64 = 3_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartSeparationReport {
    pub triangles_before: u64,
    pub connected_parts: u64,
    pub triangles_per_part: Vec<u64>,
    pub method: String,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
struct Glb {
    document: Value,
    chunks: Vec<(u32, Vec<u8>)>,
}

impl Glb {
    fn parse(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() < 20 || bytes.get(..4) != Some(GLB_MAGIC.as_slice()) {
            return Err(invalid("GLB header is missing or truncated."));
        }
        let version = read_u32(bytes, 4)?;
        let declared_length = read_u32(bytes, 8)? as usize;
        if version != 2 || declared_length != bytes.len() {
            return Err(invalid(
                "Only complete glTF binary version 2 files are supported.",
            ));
        }
        let mut cursor = 12usize;
        let mut chunks = Vec::new();
        let mut document = None;
        while cursor < bytes.len() {
            let header_end = cursor
                .checked_add(8)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| invalid("GLB chunk header is truncated."))?;
            let length = read_u32(bytes, cursor)? as usize;
            let kind = read_u32(bytes, cursor + 4)?;
            if kind == JSON_CHUNK {
                validate_part_json_size(length as u64)?;
            }
            if !length.is_multiple_of(4) {
                return Err(invalid("GLB chunk length must be 4-byte aligned."));
            }
            let end = header_end
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| invalid("GLB chunk exceeds file bounds."))?;
            let content = bytes[header_end..end].to_vec();
            if chunks.is_empty() {
                if kind != JSON_CHUNK {
                    return Err(invalid("GLB must begin with a JSON chunk."));
                }
                document = Some(
                    serde_json::from_slice(&content)
                        .map_err(|_| invalid("GLB JSON chunk is invalid."))?,
                );
            } else if kind == JSON_CHUNK {
                return Err(invalid("GLB must contain exactly one JSON chunk."));
            }
            chunks.push((kind, content));
            cursor = end;
        }
        let document = document.ok_or_else(|| invalid("GLB JSON chunk is missing."))?;
        let binary_count = chunks.iter().filter(|(kind, _)| *kind == BIN_CHUNK).count();
        if binary_count != 1 {
            return Err(unsupported(
                "Part separation requires exactly one embedded GLB binary chunk.",
            ));
        }
        Ok(Self { document, chunks })
    }

    fn binary(&self) -> &[u8] {
        self.chunks
            .iter()
            .find(|(kind, _)| *kind == BIN_CHUNK)
            .map(|(_, bytes)| bytes.as_slice())
            .unwrap_or(&[])
    }

    fn write(mut self) -> Result<Vec<u8>, CoreError> {
        let mut json_bytes = serde_json::to_vec(&self.document)?;
        pad(&mut json_bytes, b' ');
        let mut chunks = Vec::with_capacity(self.chunks.len());
        chunks.push((JSON_CHUNK, json_bytes));
        for (kind, content) in self.chunks.drain(..) {
            if kind != JSON_CHUNK {
                chunks.push((kind, content));
            }
        }
        let total = chunks
            .iter()
            .try_fold(12usize, |sum, (_, data)| {
                sum.checked_add(8)?.checked_add(data.len())
            })
            .ok_or_else(|| invalid("GLB output size overflowed."))?;
        let total_u32 = u32::try_from(total)
            .map_err(|_| unsupported("GLB output exceeds the 4 GiB format limit."))?;
        let mut bytes = Vec::with_capacity(total);
        bytes.extend_from_slice(GLB_MAGIC);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&total_u32.to_le_bytes());
        for (kind, data) in chunks {
            let length = u32::try_from(data.len())
                .map_err(|_| unsupported("GLB chunk exceeds the format limit."))?;
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(&data);
        }
        Ok(bytes)
    }
}

/// Split a static, indexed triangle GLB primitive into separate primitives by
/// connected components that share indexed vertices. This keeps the GLB asset
/// and material/attribute data intact while making each part independently
/// addressable as a mesh primitive.
pub fn separate_components(bytes: &[u8]) -> Result<(Vec<u8>, PartSeparationReport), CoreError> {
    validate_part_glb_size(bytes.len() as u64)?;
    let mut glb = Glb::parse(bytes)?;
    let bin = glb.binary().to_vec();
    let doc = &mut glb.document;
    let buffers = doc
        .get("buffers")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("GLB buffer definitions are missing."))?;
    if buffers.len() != 1 || buffers[0].get("uri").is_some() {
        return Err(unsupported(
            "Part separation currently requires one embedded GLB buffer.",
        ));
    }
    let declared_bin_length = buffers[0]
        .get("byteLength")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("GLB buffer byteLength is missing."))?
        as usize;
    if declared_bin_length > bin.len() {
        return Err(invalid("GLB buffer byteLength exceeds the BIN chunk."));
    }
    if doc
        .get("animations")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
        || doc
            .get("skins")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
    {
        return Err(unsupported(
            "Part separation currently supports static, unskinned meshes only.",
        ));
    }
    if doc
        .get("extensionsRequired")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
    {
        return Err(unsupported(
            "GLB uses required extensions that this part separator does not interpret.",
        ));
    }
    let meshes = doc
        .get("meshes")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("GLB meshes are missing."))?;
    if meshes.len() != 1 {
        return Err(unsupported(
            "Part separation currently supports one mesh per GLB.",
        ));
    }
    let mesh = &meshes[0];
    let primitives = mesh
        .get("primitives")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("Mesh primitives are missing."))?;
    if primitives.len() != 1 {
        return Err(unsupported(
            "Part separation currently supports one primitive per GLB.",
        ));
    }
    let primitive = &primitives[0];
    if primitive.get("mode").and_then(Value::as_u64).unwrap_or(4) != 4 {
        return Err(unsupported(
            "Part separation only supports TRIANGLES primitives.",
        ));
    }
    if primitive.get("targets").is_some() || primitive.get("extensions").is_some() {
        return Err(unsupported(
            "Morph targets and primitive extensions are not supported by part separation.",
        ));
    }
    let attributes = primitive
        .get("attributes")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("Primitive attributes are missing."))?;
    if attributes
        .keys()
        .any(|name| name.starts_with("JOINTS_") || name.starts_with("WEIGHTS_"))
    {
        return Err(unsupported(
            "Skinned vertex attributes are not supported by part separation.",
        ));
    }
    let positions_index = attributes
        .get("POSITION")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("Primitive POSITION attribute is missing."))?
        as usize;
    let positions_count = read_positions(doc, &bin[..declared_bin_length], positions_index)?;
    let indices_index = primitive
        .get("indices")
        .and_then(Value::as_u64)
        .ok_or_else(|| unsupported("Part separation requires an indexed primitive."))?
        as usize;
    let indices = read_indices(doc, &bin[..declared_bin_length], indices_index)?;
    if indices.is_empty() || !indices.len().is_multiple_of(3) {
        return Err(invalid(
            "Triangle index count must be nonzero and divisible by three.",
        ));
    }
    for index in &indices {
        if *index as usize >= positions_count {
            return Err(invalid(
                "Primitive index references a vertex outside POSITION.",
            ));
        }
    }
    let triangle_count = indices.len() / 3;
    let mut parent: Vec<usize> = (0..triangle_count).collect();
    let mut rank = vec![0u8; triangle_count];
    let mut first_for_vertex = vec![usize::MAX; positions_count];
    for triangle in 0..triangle_count {
        for vertex in &indices[triangle * 3..triangle * 3 + 3] {
            let vertex_index = *vertex as usize;
            let first = first_for_vertex[vertex_index];
            if first == usize::MAX {
                first_for_vertex[vertex_index] = triangle;
            } else {
                union(&mut parent, &mut rank, triangle, first);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
    let mut component_count = 0;
    for (triangle, tri) in indices.chunks_exact(3).enumerate() {
        let root = find(&mut parent, triangle);
        match groups.entry(root) {
            Entry::Occupied(mut entry) => entry.get_mut().extend_from_slice(tri),
            Entry::Vacant(entry) => {
                if component_count >= MAX_PARTS {
                    return Err(unsupported(format!(
                        "Part separation would exceed the limit of {MAX_PARTS} connected parts."
                    )));
                }
                component_count += 1;
                entry.insert(tri.to_vec());
            }
        }
    }
    let mut grouped: Vec<Vec<u32>> = groups.into_values().collect();
    grouped.sort_by_key(|part| part.iter().copied().min().unwrap_or(u32::MAX));
    let triangles_per_part: Vec<u64> = grouped.iter().map(|part| (part.len() / 3) as u64).collect();
    drop(indices);
    drop(parent);
    drop(rank);
    drop(first_for_vertex);
    let mut output_bin = bin;
    let mut output_primitives = Vec::with_capacity(grouped.len());
    let mut buffer_views = doc
        .get("bufferViews")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut accessors = doc
        .get("accessors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for part_indices in &grouped {
        while !output_bin.len().is_multiple_of(4) {
            output_bin.push(0);
        }
        let offset = output_bin.len();
        for index in part_indices {
            output_bin.extend_from_slice(&index.to_le_bytes());
        }
        let byte_length = output_bin.len() - offset;
        let view_index = buffer_views.len();
        buffer_views.push(
            json!({"buffer": 0, "byteOffset": offset, "byteLength": byte_length, "target": 34963}),
        );
        let accessor_index = accessors.len();
        let (minimum, maximum) = part_indices
            .iter()
            .fold((u32::MAX, u32::MIN), |(min, max), i| {
                (min.min(*i), max.max(*i))
            });
        accessors.push(json!({"bufferView": view_index, "byteOffset": 0, "componentType": 5125, "count": part_indices.len(), "type": "SCALAR", "min": [minimum], "max": [maximum]}));
        let mut separated = primitive.clone();
        separated["indices"] = json!(accessor_index);
        output_primitives.push(separated);
    }
    doc["bufferViews"] = Value::Array(buffer_views);
    doc["accessors"] = Value::Array(accessors);
    doc["meshes"][0]["primitives"] = Value::Array(output_primitives);
    doc.get_mut("buffers")
        .and_then(Value::as_array_mut)
        .and_then(|buffers| buffers.first_mut())
        .ok_or_else(|| invalid("GLB buffer definitions are missing."))?["byteLength"] =
        json!(output_bin.len());
    if let Some((_, data)) = glb.chunks.iter_mut().find(|(kind, _)| *kind == BIN_CHUNK) {
        *data = output_bin;
    }
    let report = PartSeparationReport {
        triangles_before: triangle_count as u64,
        connected_parts: grouped.len() as u64,
        triangles_per_part,
        method: "shared_indexed_vertex_components".into(),
        warnings: vec![
            "Connectivity is determined by shared vertex indices; touching surfaces with duplicated vertices are separate parts.".into(),
            "Parts are emitted as separate TRIANGLES primitives in one GLB mesh. Original vertex streams and materials are retained; source buffer data remains in the binary chunk.".into(),
        ],
    };
    Ok((glb.write()?, report))
}

fn read_positions(doc: &Value, bin: &[u8], accessor_index: usize) -> Result<usize, CoreError> {
    let accessor = array(doc, "accessors")
        .get(accessor_index)
        .ok_or_else(|| invalid("POSITION accessor does not exist."))?;
    if accessor.get("type").and_then(Value::as_str) != Some("VEC3")
        || accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        || accessor.get("sparse").is_some()
        || accessor.get("extensions").is_some()
    {
        return Err(unsupported(
            "POSITION must be a non-sparse FLOAT VEC3 accessor.",
        ));
    }
    let view_index = accessor
        .get("bufferView")
        .and_then(Value::as_u64)
        .ok_or_else(|| unsupported("POSITION must reference a bufferView."))?
        as usize;
    let view = array(doc, "bufferViews")
        .get(view_index)
        .ok_or_else(|| invalid("POSITION bufferView does not exist."))?;
    if view.get("buffer").and_then(Value::as_u64) != Some(0) || view.get("extensions").is_some() {
        return Err(unsupported("POSITION must use the embedded GLB buffer."));
    }
    let count = accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("POSITION count is missing."))?;
    let count = validate_part_vertex_count(count)?;
    let view_start = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let view_length =
        view.get("byteLength")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("POSITION bufferView byteLength is missing."))? as usize;
    let view_end = view_start
        .checked_add(view_length)
        .filter(|end| *end <= bin.len())
        .ok_or_else(|| invalid("POSITION bufferView exceeds BIN chunk."))?;
    let stride = view
        .get("byteStride")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(12);
    if stride < 12 || !stride.is_multiple_of(4) {
        return Err(invalid("POSITION byteStride is invalid."));
    }
    let start = view_start
        .checked_add(
            accessor
                .get("byteOffset")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
        )
        .ok_or_else(|| invalid("POSITION offset overflowed."))?;
    let available = view_end
        .checked_sub(start)
        .ok_or_else(|| invalid("POSITION accessor starts outside its bufferView."))?;
    if count == 0 || count - 1 > available.saturating_sub(12) / stride {
        return Err(invalid("POSITION accessor exceeds its bufferView."));
    }
    for i in 0..count {
        let offset = start
            .checked_add(
                i.checked_mul(stride)
                    .ok_or_else(|| invalid("POSITION offset overflowed."))?,
            )
            .ok_or_else(|| invalid("POSITION offset overflowed."))?;
        let end = offset
            .checked_add(12)
            .filter(|end| *end <= view_end)
            .ok_or_else(|| invalid("POSITION accessor exceeds BIN chunk."))?;
        for component in 0..3 {
            let pos = offset + component * 4;
            let value = f32::from_le_bytes(bin[pos..pos + 4].try_into().expect("four bytes"));
            if !value.is_finite() {
                return Err(invalid("POSITION contains a non-finite value."));
            }
        }
        debug_assert_eq!(end - offset, 12);
    }
    Ok(count)
}

fn read_indices(doc: &Value, bin: &[u8], accessor_index: usize) -> Result<Vec<u32>, CoreError> {
    let accessor = array(doc, "accessors")
        .get(accessor_index)
        .ok_or_else(|| invalid("Index accessor does not exist."))?;
    if accessor.get("type").and_then(Value::as_str) != Some("SCALAR")
        || accessor.get("sparse").is_some()
        || accessor.get("extensions").is_some()
    {
        return Err(unsupported("Indices must be a non-sparse SCALAR accessor."));
    }
    let view_index = accessor
        .get("bufferView")
        .and_then(Value::as_u64)
        .ok_or_else(|| unsupported("Indices must reference a bufferView."))?
        as usize;
    let view = array(doc, "bufferViews")
        .get(view_index)
        .ok_or_else(|| invalid("Index bufferView does not exist."))?;
    if view.get("buffer").and_then(Value::as_u64) != Some(0) || view.get("extensions").is_some() {
        return Err(unsupported("Indices must use the embedded GLB buffer."));
    }
    let component_type = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("Index componentType is missing."))?;
    let element_size = match component_type {
        5121 => 1usize,
        5123 => 2,
        5125 => 4,
        _ => {
            return Err(unsupported(
                "Indices must use UNSIGNED_BYTE, UNSIGNED_SHORT, or UNSIGNED_INT.",
            ));
        }
    };
    let count = accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("Index count is missing."))?;
    if count == 0 || !count.is_multiple_of(3) {
        return Err(invalid(
            "Triangle index count must be nonzero and divisible by three.",
        ));
    }
    let count = validate_part_index_count(count)?;
    let view_start = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let view_length =
        view.get("byteLength")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("Index bufferView byteLength is missing."))? as usize;
    let view_end = view_start
        .checked_add(view_length)
        .filter(|end| *end <= bin.len())
        .ok_or_else(|| invalid("Index bufferView exceeds BIN chunk."))?;
    if view.get("byteStride").is_some() {
        return Err(unsupported(
            "Index bufferViews with byteStride are not supported.",
        ));
    }
    let stride = element_size;
    let start = view_start
        .checked_add(
            accessor
                .get("byteOffset")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
        )
        .ok_or_else(|| invalid("Index offset overflowed."))?;
    let available = view_end
        .checked_sub(start)
        .ok_or_else(|| invalid("Index accessor starts outside its bufferView."))?;
    if count - 1 > available.saturating_sub(element_size) / stride {
        return Err(invalid("Index accessor exceeds its bufferView."));
    }
    let mut indices = Vec::with_capacity(count);
    for i in 0..count {
        let offset = start
            .checked_add(
                i.checked_mul(stride)
                    .ok_or_else(|| invalid("Index offset overflowed."))?,
            )
            .ok_or_else(|| invalid("Index offset overflowed."))?;
        let end = offset
            .checked_add(element_size)
            .filter(|end| *end <= view_end)
            .ok_or_else(|| invalid("Index accessor exceeds BIN chunk."))?;
        let data = &bin[offset..end];
        indices.push(match component_type {
            5121 => data[0] as u32,
            5123 => u16::from_le_bytes(data.try_into().expect("two bytes")) as u32,
            5125 => u32::from_le_bytes(data.try_into().expect("four bytes")),
            _ => unreachable!(),
        });
    }
    Ok(indices)
}

fn find(parent: &mut [usize], value: usize) -> usize {
    if parent[value] != value {
        parent[value] = find(parent, parent[value]);
    }
    parent[value]
}

fn union(parent: &mut [usize], rank: &mut [u8], a: usize, b: usize) {
    let mut root_a = find(parent, a);
    let mut root_b = find(parent, b);
    if root_a == root_b {
        return;
    }
    if rank[root_a] < rank[root_b] {
        std::mem::swap(&mut root_a, &mut root_b);
    }
    parent[root_b] = root_a;
    if rank[root_a] == rank[root_b] {
        rank[root_a] = rank[root_a].saturating_add(1);
    }
}

pub(crate) fn validate_part_glb_size(size: u64) -> Result<(), CoreError> {
    if size > MAX_PART_GLB_BYTES {
        return Err(unsupported(format!(
            "Part separation supports GLB files up to {} MiB.",
            MAX_PART_GLB_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

fn validate_part_json_size(size: u64) -> Result<(), CoreError> {
    if size > MAX_PART_JSON_BYTES {
        return Err(unsupported(format!(
            "Part separation supports JSON chunks up to {} MiB.",
            MAX_PART_JSON_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

fn validate_part_vertex_count(count: u64) -> Result<usize, CoreError> {
    if count == 0 {
        return Err(invalid("POSITION cannot be empty."));
    }
    if count > MAX_PART_VERTICES {
        return Err(unsupported(format!(
            "Part separation supports at most {MAX_PART_VERTICES} POSITION vertices."
        )));
    }
    usize::try_from(count)
        .map_err(|_| unsupported("POSITION vertex count exceeds this platform's limits."))
}

fn validate_part_index_count(count: u64) -> Result<usize, CoreError> {
    if count > MAX_PART_INDICES {
        return Err(unsupported(format!(
            "Part separation supports at most {MAX_PART_INDICES} indices."
        )));
    }
    usize::try_from(count)
        .map_err(|_| unsupported("Triangle index count exceeds this platform's limits."))
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, CoreError> {
    let end = offset
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| invalid("GLB header is truncated."))?;
    Ok(u32::from_le_bytes(
        bytes[offset..end].try_into().expect("four bytes"),
    ))
}

fn pad(bytes: &mut Vec<u8>, value: u8) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(value);
    }
}
fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::InvalidInput(message.into())
}
fn unsupported(message: impl Into<String>) -> CoreError {
    CoreError::OperationUnavailable(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(mut doc: Value, mut bin: Vec<u8>) -> Vec<u8> {
        pad(&mut bin, 0);
        doc["buffers"] = json!([{"byteLength": bin.len()}]);
        let mut json_bytes = serde_json::to_vec(&doc).unwrap();
        pad(&mut json_bytes, b' ');
        let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
        let mut out = Vec::new();
        out.extend_from_slice(GLB_MAGIC);
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&JSON_CHUNK.to_le_bytes());
        out.extend_from_slice(&json_bytes);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&BIN_CHUNK.to_le_bytes());
        out.extend_from_slice(&bin);
        out
    }

    fn disconnected_triangles(count: usize) -> Vec<u8> {
        let mut bin = Vec::new();
        for triangle in 0..count {
            let x = triangle as f32 * 4.0;
            for point in [[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]] {
                for component in point {
                    bin.extend_from_slice(&component.to_le_bytes());
                }
            }
        }
        let index_offset = bin.len();
        for index in 0..count * 3 {
            bin.extend_from_slice(&(index as u32).to_le_bytes());
        }
        pack(
            json!({
                "asset":{"version":"2.0"},
                "bufferViews":[
                    {"buffer":0,"byteOffset":0,"byteLength":count * 36,"byteStride":12,"target":34962},
                    {"buffer":0,"byteOffset":index_offset,"byteLength":count * 12,"target":34963}
                ],
                "accessors":[
                    {"bufferView":0,"componentType":5126,"count":count * 3,"type":"VEC3"},
                    {"bufferView":1,"componentType":5125,"count":count * 3,"type":"SCALAR"}
                ],
                "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
                "nodes":[{"mesh":0}], "scenes":[{"nodes":[0]}], "scene":0
            }),
            bin,
        )
    }

    fn two_components() -> Vec<u8> {
        let mut bin = Vec::new();
        for point in [
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [10.0, 0.0, 0.0],
            [11.0, 0.0, 0.0],
            [10.0, 1.0, 0.0],
        ] {
            for c in point {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let index_offset = bin.len();
        for i in [0u16, 1, 2, 3, 4, 5] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        pack(
            json!({
                "asset":{"version":"2.0"},
                "bufferViews":[
                    {"buffer":0,"byteOffset":0,"byteLength":72,"byteStride":12,"target":34962},
                    {"buffer":0,"byteOffset":index_offset,"byteLength":12,"target":34963}
                ],
                "accessors":[
                    {"bufferView":0,"componentType":5126,"count":6,"type":"VEC3"},
                    {"bufferView":1,"componentType":5123,"count":6,"type":"SCALAR"}
                ],
                "materials":[{"name":"kept"}],
                "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1,"material":0}]}],
                "nodes":[{"mesh":0}], "scenes":[{"nodes":[0]}], "scene":0
            }),
            bin,
        )
    }

    #[test]
    fn enforces_memory_limits_before_large_allocations() {
        assert!(validate_part_glb_size(MAX_PART_GLB_BYTES).is_ok());
        assert!(validate_part_glb_size(MAX_PART_GLB_BYTES + 1).is_err());
        assert!(validate_part_json_size(MAX_PART_JSON_BYTES).is_ok());
        assert!(validate_part_json_size(MAX_PART_JSON_BYTES + 1).is_err());
        assert_eq!(
            validate_part_vertex_count(MAX_PART_VERTICES).unwrap(),
            MAX_PART_VERTICES as usize
        );
        assert!(validate_part_vertex_count(MAX_PART_VERTICES + 1).is_err());
        assert_eq!(
            validate_part_index_count(MAX_PART_INDICES).unwrap(),
            MAX_PART_INDICES as usize
        );
        assert!(validate_part_index_count(MAX_PART_INDICES + 1).is_err());
    }

    #[test]
    fn rejects_more_connected_parts_than_the_bounded_output_allows() {
        let error = separate_components(&disconnected_triangles(MAX_PARTS + 1)).unwrap_err();
        assert!(error.to_string().contains("connected parts"));
    }

    #[test]
    fn splits_disconnected_triangles_and_preserves_material() {
        let (output, report) = separate_components(&two_components()).unwrap();
        assert_eq!(report.triangles_before, 2);
        assert_eq!(report.connected_parts, 2);
        assert_eq!(report.triangles_per_part, vec![1, 1]);
        let parsed = Glb::parse(&output).unwrap();
        let primitives = parsed.document["meshes"][0]["primitives"]
            .as_array()
            .unwrap();
        assert_eq!(primitives.len(), 2);
        assert_eq!(primitives[0]["material"], 0);
        assert_eq!(primitives[1]["material"], 0);
        assert_eq!(parsed.document["materials"][0]["name"], "kept");
    }

    #[test]
    fn triangles_sharing_an_indexed_vertex_are_one_part() {
        let mut glb = Glb::parse(&two_components()).unwrap();
        let mut bin = glb.binary().to_vec();
        let index_offset = 72;
        let new_indices = [0u16, 1, 2, 2, 1, 3];
        bin.truncate(index_offset);
        for i in new_indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        glb.document["bufferViews"][1]["byteLength"] = json!(12);
        glb.document["buffers"][0]["byteLength"] = json!(bin.len());
        glb.chunks
            .iter_mut()
            .find(|(kind, _)| *kind == BIN_CHUNK)
            .unwrap()
            .1 = bin;
        let bytes = glb.write().unwrap();
        let (_, report) = separate_components(&bytes).unwrap();
        assert_eq!(report.connected_parts, 1);
    }

    #[test]
    fn rejects_bad_index_and_non_triangles() {
        let mut glb = Glb::parse(&two_components()).unwrap();
        let mut bin = glb.binary().to_vec();
        bin[72..74].copy_from_slice(&99u16.to_le_bytes());
        glb.chunks
            .iter_mut()
            .find(|(kind, _)| *kind == BIN_CHUNK)
            .unwrap()
            .1 = bin;
        let invalid = glb.write().unwrap();
        assert!(separate_components(&invalid).is_err());

        let mut glb = Glb::parse(&two_components()).unwrap();
        glb.document["meshes"][0]["primitives"][0]["mode"] = json!(1);
        assert!(separate_components(&glb.write().unwrap()).is_err());
    }
}
