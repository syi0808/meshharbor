//! Selective xatlas integration for safe, static GLB UV atlas rebaking.
//!
//! This operation does not simplify geometry or transfer appearance between
//! different meshes. It unwraps one supported, already textured primitive,
//! duplicates its vertex streams through xatlas' source-vertex map, and
//! rebakes its embedded base-color and metallic/roughness maps from the same
//! mesh's existing `TEXCOORD_0`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{error::Error, fmt};

const GLB_MAGIC: &[u8; 4] = b"glTF";
const JSON_CHUNK: u32 = 0x4e4f_534a;
const BIN_CHUNK: u32 = 0x004e_4942;
const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const MAX_VERTICES: usize = 5_000_000;
const MAX_INDICES: usize = 15_000_000;
const MAX_PNG_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtlasBakeError {
    InvalidInput(String),
    Unsupported(String),
    Native(String),
    Output(String),
}

impl fmt::Display for AtlasBakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "Invalid GLB: {message}"),
            Self::Unsupported(message) => write!(
                formatter,
                "Atlas baking is not supported for this asset: {message}"
            ),
            Self::Native(message) => write!(formatter, "Atlas generation failed: {message}"),
            Self::Output(message) => write!(formatter, "Could not encode atlas GLB: {message}"),
        }
    }
}

impl Error for AtlasBakeError {}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AtlasBakeOptions {
    /// xatlas packing resolution hint. Output width/height may be smaller.
    pub resolution: u32,
    /// Number of atlas texels reserved for each chart's filtering gutter.
    pub padding: u32,
}

impl Default for AtlasBakeOptions {
    fn default() -> Self {
        Self {
            resolution: 1024,
            padding: 8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AtlasBakeReport {
    pub source_triangles: u64,
    pub output_triangles: u64,
    pub source_vertices: u64,
    pub output_vertices: u64,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub chart_count: u32,
    pub covered_texels: u64,
    pub gutter_texels: u64,
    pub preserved_attributes: Vec<String>,
    pub rebaked_maps: Vec<String>,
    pub warnings: Vec<String>,
}

/// Geometry and UV streams from the narrow texture-projection source format.
/// The caller may pass these positions and indices to a geometry-only remesher.
#[derive(Debug, Clone)]
pub struct ProjectionSourceMesh {
    pub positions: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectedMeshBakeReport {
    pub source_triangles: u64,
    pub target_triangles: u64,
    pub output_triangles: u64,
    pub output_vertices: u64,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub chart_count: u32,
    pub covered_texels: u64,
    pub gutter_texels: u64,
    pub projection_attempted_texels: u64,
    pub projection_covered_texels: u64,
    pub projection_coverage: f32,
    pub ambiguous_texels: u64,
    pub maximum_projection_distance: f32,
    pub projection_distance_limit: f32,
    pub rebaked_maps: Vec<String>,
    pub warnings: Vec<String>,
}

/// Target geometry with a new atlas and projected base-color/metallic-roughness
/// maps. The result is not a texture-quality certification.
#[derive(Debug, Clone)]
pub struct ProjectedMeshBake {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub base_color_png: Vec<u8>,
    pub metallic_roughness_png: Vec<u8>,
    pub base_color_factor: [f32; 4],
    pub metallic_factor: f32,
    pub roughness_factor: f32,
    pub double_sided: bool,
    pub report: ProjectedMeshBakeReport,
}

/// Result for one bounded closest-surface projection query. This primitive is
/// used by the projected rebake path and deliberately does not imply a bake.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceProjection {
    pub triangle: u32,
    pub barycentric: [f32; 3],
    pub position: [f32; 3],
    pub distance: f32,
    pub normal_dot: f32,
    /// A second, nearly-equidistant hit with an opposing normal is ambiguous.
    pub ambiguous: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct ProjectionCandidate {
    distance2: f32,
    triangle: u32,
    barycentric: [f32; 3],
    position: [f32; 3],
    normal_dot: f32,
}

/// Project one point onto an indexed triangle surface. The search is bounded
/// by `max_distance`; opposing near-equal hits are reported as ambiguous so a
/// caller can fail closed on thin shells. This is exposed separately to make
/// projection semantics testable without encoding a texture atlas.
#[cfg(test)]
fn project_point_to_surface(
    point: [f32; 3],
    target_normal: [f32; 3],
    source_positions: &[[f32; 3]],
    source_indices: &[u32],
    max_distance: f32,
) -> Result<SurfaceProjection, AtlasBakeError> {
    if !point
        .iter()
        .chain(target_normal.iter())
        .all(|v| v.is_finite())
        || !max_distance.is_finite()
        || max_distance <= 0.0
        || source_positions.is_empty()
        || source_indices.is_empty()
        || !source_indices.len().is_multiple_of(3)
        || source_indices.len() / 3 > u32::MAX as usize
        || source_positions.iter().flatten().any(|v| !v.is_finite())
        || source_indices
            .iter()
            .any(|i| *i as usize >= source_positions.len())
    {
        return Err(AtlasBakeError::InvalidInput(
            "invalid projection query or source triangle mesh".into(),
        ));
    }
    let norm = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let tn = norm(target_normal);
    if tn <= 1e-12 {
        return Err(AtlasBakeError::InvalidInput(
            "target normal must be non-zero".into(),
        ));
    }
    let target_normal = [
        target_normal[0] / tn,
        target_normal[1] / tn,
        target_normal[2] / tn,
    ];
    let mut best: Option<ProjectionCandidate> = None;
    let mut nearest_opposing = f32::INFINITY;
    for (face, tri) in source_indices.chunks_exact(3).enumerate() {
        let a = source_positions[tri[0] as usize];
        let b = source_positions[tri[1] as usize];
        let c = source_positions[tri[2] as usize];
        let ab = sub3(b, a);
        let ac = sub3(c, a);
        let face_n = normalize3(cross3(ab, ac));
        if face_n.is_none() {
            continue;
        }
        let n = face_n.unwrap();
        let (q, bary) = closest_point_triangle(point, a, b, c);
        let d2 = dot3(sub3(q, point), sub3(q, point));
        if d2 > max_distance * max_distance {
            continue;
        }
        let nd = dot3(target_normal, n);
        if nd < -0.25 {
            nearest_opposing = nearest_opposing.min(d2);
        }
        if best.is_none_or(|candidate| d2 < candidate.distance2) {
            best = Some(ProjectionCandidate {
                distance2: d2,
                triangle: face as u32,
                barycentric: bary,
                position: q,
                normal_dot: nd,
            });
        }
    }
    let Some(candidate) = best else {
        return Err(AtlasBakeError::Unsupported(
            "no source surface lies within the projection distance limit".into(),
        ));
    };
    let ProjectionCandidate {
        distance2: d2,
        triangle,
        barycentric,
        position,
        normal_dot,
    } = candidate;
    if normal_dot < 0.25 {
        return Err(AtlasBakeError::Unsupported(
            "nearest source surface fails the minimum facing-direction gate".into(),
        ));
    }
    let distance = d2.sqrt();
    let ambiguous = nearest_opposing.is_finite()
        && (nearest_opposing.sqrt() - distance).abs() <= max_distance * 0.1;
    Ok(SurfaceProjection {
        triangle,
        barycentric,
        position,
        distance,
        normal_dot,
        ambiguous,
    })
}

#[cfg(test)]
fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[cfg(test)]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[cfg(test)]
fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[cfg(test)]
fn normalize3(a: [f32; 3]) -> Option<[f32; 3]> {
    let n = dot3(a, a).sqrt();
    (n > 1e-12).then(|| [a[0] / n, a[1] / n, a[2] / n])
}
#[cfg(test)]
fn closest_point_triangle(
    p: [f32; 3],
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    let ab = sub3(b, a);
    let ac = sub3(c, a);
    let ap = sub3(p, a);
    let d1 = dot3(ab, ap);
    let d2 = dot3(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = sub3(p, b);
    let d3 = dot3(ab, bp);
    let d4 = dot3(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (add_scaled(a, ab, v), [1.0 - v, v, 0.0]);
    }
    let cp = sub3(p, c);
    let d5 = dot3(ab, cp);
    let d6 = dot3(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (add_scaled(a, ac, w), [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (add_scaled(b, sub3(c, b), w), [0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (add_scaled(add_scaled(a, ab, v), ac, w), [1.0 - v - w, v, w])
}
#[cfg(test)]
fn add_scaled(a: [f32; 3], b: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

/// Generate atlas UVs and rebake the supported embedded PBR textures.
///
/// Static attributes other than `TEXCOORD_0` are duplicated byte-for-byte via
/// xatlas' xref map. The only geometry change is vertex splitting for atlas
/// seams plus the corresponding new triangle index buffer. Meshoptimizer
/// simplification is intentionally a separate operation.
pub fn bake_glb(
    bytes: &[u8],
    options: AtlasBakeOptions,
) -> Result<(Vec<u8>, AtlasBakeReport), AtlasBakeError> {
    validate_options(options)?;
    let mut glb = Glb::parse(bytes)?;
    let snapshot = validate_supported(&glb.document)?;

    let position_data = read_accessor(&glb.document, &glb.binary, snapshot.position_accessor)?;
    require_accessor(
        &position_data,
        "POSITION",
        5126,
        "VEC3",
        snapshot.source_vertices,
    )?;
    let positions = decode_vec3(&position_data.bytes)?;

    let uv_data = read_accessor(&glb.document, &glb.binary, snapshot.uv_accessor)?;
    require_accessor(
        &uv_data,
        "TEXCOORD_0",
        5126,
        "VEC2",
        snapshot.source_vertices,
    )?;
    let source_uv = decode_vec2(&uv_data.bytes)?;

    let normals = if let Some(normal_accessor) = snapshot.normal_accessor {
        let normal_data = read_accessor(&glb.document, &glb.binary, normal_accessor)?;
        require_accessor(
            &normal_data,
            "NORMAL",
            5126,
            "VEC3",
            snapshot.source_vertices,
        )?;
        Some(decode_vec3(&normal_data.bytes)?)
    } else {
        None
    };

    let indices = if let Some(index_accessor) = snapshot.index_accessor {
        read_indices(&glb.document, &glb.binary, index_accessor)?
    } else {
        (0..u32::try_from(snapshot.source_vertices).map_err(|_| invalid("too many vertices"))?)
            .collect()
    };
    if indices.is_empty() || !indices.len().is_multiple_of(3) || indices.len() > MAX_INDICES {
        return Err(AtlasBakeError::InvalidInput(
            "the primitive must contain a bounded, non-empty triangle index stream".into(),
        ));
    }
    if indices
        .iter()
        .any(|index| *index as usize >= snapshot.source_vertices)
    {
        return Err(invalid("an index points outside the POSITION accessor"));
    }

    let base_png = read_png_image(&glb.document, &glb.binary, snapshot.base_image)?;
    let mr_png = read_png_image(&glb.document, &glb.binary, snapshot.mr_image)?;
    let native = meshharbor_xatlas_sys::unwrap_and_bake(
        meshharbor_xatlas_sys::AtlasInput {
            positions: &positions,
            normals: normals.as_deref(),
            original_uv: &source_uv,
            indices: &indices,
            base_png: &base_png,
            mr_png: &mr_png,
        },
        meshharbor_xatlas_sys::AtlasSettings {
            resolution: options.resolution,
            padding: options.padding,
        },
    )
    .map_err(AtlasBakeError::Native)?;

    if native.xref.len() > MAX_INDICES
        || native.xref.len() != native.uv.len()
        || !native.indices.len().is_multiple_of(3)
        || native.indices.len() != indices.len()
        || native
            .xref
            .iter()
            .any(|index| *index as usize >= snapshot.source_vertices)
        || native
            .indices
            .iter()
            .any(|index| *index as usize >= native.xref.len())
    {
        return Err(AtlasBakeError::Native(
            "xatlas returned inconsistent vertex or index arrays".into(),
        ));
    }
    if native
        .uv
        .iter()
        .flatten()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err(AtlasBakeError::Native(
            "xatlas returned non-finite or out-of-range atlas coordinates".into(),
        ));
    }

    let attributes = glb.document["meshes"][0]["primitives"][0]["attributes"]
        .as_object()
        .ok_or_else(|| invalid("primitive attributes are missing"))?
        .clone();
    let mut preserved_attributes: Vec<String> = attributes.keys().cloned().collect();
    preserved_attributes.sort();

    // Rewrite every vertex stream, not just POSITION/NORMAL/UV, so custom,
    // color, joint, weight, and secondary UV data remain aligned after seams split.
    let mut replaced_attributes = Map::new();
    for (semantic, accessor_value) in &attributes {
        let accessor_index = as_index(accessor_value, "attribute accessor")?;
        let (new_view, new_accessor) = if semantic == "TEXCOORD_0" {
            let mut raw = Vec::with_capacity(native.uv.len() * 8);
            for pair in &native.uv {
                raw.extend_from_slice(&pair[0].to_le_bytes());
                raw.extend_from_slice(&pair[1].to_le_bytes());
            }
            let view = append_view(&mut glb.document, &mut glb.binary, &raw, Some(34962))?;
            let accessor =
                append_new_accessor(&mut glb.document, view, native.uv.len(), 5126, "VEC2", None)?;
            (view, accessor)
        } else {
            let data = read_accessor(&glb.document, &glb.binary, accessor_index)?;
            if data.count != snapshot.source_vertices {
                return Err(AtlasBakeError::Unsupported(format!(
                    "attribute {semantic} has {} entries but POSITION has {}",
                    data.count, snapshot.source_vertices
                )));
            }
            if !matches!(data.type_name.as_str(), "SCALAR" | "VEC2" | "VEC3" | "VEC4") {
                return Err(AtlasBakeError::Unsupported(format!(
                    "attribute {semantic} uses unsupported accessor type {}",
                    data.type_name
                )));
            }
            let mut remapped = Vec::with_capacity(native.xref.len() * data.element_size);
            for source in &native.xref {
                let start = (*source as usize)
                    .checked_mul(data.element_size)
                    .ok_or_else(|| invalid("attribute offset overflow"))?;
                let end = start
                    .checked_add(data.element_size)
                    .filter(|end| *end <= data.bytes.len())
                    .ok_or_else(|| invalid(format!("attribute {semantic} is truncated")))?;
                remapped.extend_from_slice(&data.bytes[start..end]);
            }
            let view = append_view(&mut glb.document, &mut glb.binary, &remapped, Some(34962))?;
            let accessor =
                append_accessor_like(&mut glb.document, accessor_index, view, native.xref.len())?;
            (view, accessor)
        };
        let _ = new_view;
        replaced_attributes.insert(semantic.clone(), json!(new_accessor));
    }
    glb.document["meshes"][0]["primitives"][0]["attributes"] = Value::Object(replaced_attributes);

    let index_component_type = if native.xref.len() <= u16::MAX as usize + 1 {
        5123
    } else {
        5125
    };
    let mut index_bytes =
        Vec::with_capacity(native.indices.len() * if index_component_type == 5123 { 2 } else { 4 });
    if index_component_type == 5123 {
        for index in &native.indices {
            index_bytes.extend_from_slice(
                &u16::try_from(*index)
                    .map_err(|_| {
                        AtlasBakeError::Native(
                            "xatlas index exceeded the 16-bit vertex range".into(),
                        )
                    })?
                    .to_le_bytes(),
            );
        }
    } else {
        for index in &native.indices {
            index_bytes.extend_from_slice(&index.to_le_bytes());
        }
    }
    let index_view = append_view(
        &mut glb.document,
        &mut glb.binary,
        &index_bytes,
        Some(34963),
    )?;
    let new_indices = append_new_accessor(
        &mut glb.document,
        index_view,
        native.indices.len(),
        index_component_type,
        "SCALAR",
        None,
    )?;
    glb.document["meshes"][0]["primitives"][0]["indices"] = json!(new_indices);

    // Clone image and texture records instead of modifying source resources that
    // may be referenced by a different material elsewhere in the document.
    let base_view = append_view(
        &mut glb.document,
        &mut glb.binary,
        &native.base_color_png,
        None,
    )?;
    let mr_view = append_view(
        &mut glb.document,
        &mut glb.binary,
        &native.metallic_roughness_png,
        None,
    )?;
    let base_texture_index = append_atlas_texture(
        &mut glb.document,
        snapshot.base_texture,
        snapshot.base_image,
        base_view,
    )?;
    let mr_texture_index = append_atlas_texture(
        &mut glb.document,
        snapshot.mr_texture,
        snapshot.mr_image,
        mr_view,
    )?;
    glb.document["materials"][snapshot.material]["pbrMetallicRoughness"]["baseColorTexture"]["index"] =
        json!(base_texture_index);
    glb.document["materials"][snapshot.material]["pbrMetallicRoughness"]["metallicRoughnessTexture"]
        ["index"] = json!(mr_texture_index);

    if let Some(buffer) = glb
        .document
        .get_mut("buffers")
        .and_then(Value::as_array_mut)
        .and_then(|buffers| buffers.get_mut(0))
    {
        buffer["byteLength"] = json!(glb.binary.len());
    } else {
        return Err(invalid("the GLB embedded buffer entry is missing"));
    }

    let report = AtlasBakeReport {
        source_triangles: (indices.len() / 3) as u64,
        output_triangles: (native.indices.len() / 3) as u64,
        source_vertices: snapshot.source_vertices as u64,
        output_vertices: native.xref.len() as u64,
        atlas_width: native.width,
        atlas_height: native.height,
        chart_count: native.chart_count,
        covered_texels: native.covered_texels,
        gutter_texels: native.gutter_texels,
        preserved_attributes,
        rebaked_maps: vec!["baseColorTexture".into(), "metallicRoughnessTexture".into()],
        warnings: vec![
            "Texture transfer samples the original UV-mapped surface; this operation does not simplify or transfer onto different geometry.".into(),
            "Normal, occlusion, and emissive texture baking is not implemented; assets using those maps are rejected.".into(),
        ],
    };
    Ok((glb.write()?, report))
}

/// Read the narrow source subset accepted for retopology appearance projection.
/// This intentionally rejects extra vertex streams and scene transforms so
/// ignored attributes cannot silently disappear from a retopologized asset.
pub fn extract_projection_source_mesh(
    bytes: &[u8],
) -> Result<ProjectionSourceMesh, AtlasBakeError> {
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(unsupported(
            "projection source GLB exceeds the 64 MiB limit",
        ));
    }
    let glb = Glb::parse(bytes)?;
    if !glb.other_chunks.is_empty() {
        return Err(unsupported(
            "unknown GLB chunks are not supported for projection",
        ));
    }
    let snapshot = validate_supported(&glb.document)?;
    validate_projection_scene(&glb.document)?;
    let attributes = glb.document["meshes"][0]["primitives"][0]["attributes"]
        .as_object()
        .ok_or_else(|| invalid("primitive attributes are missing"))?;
    if attributes
        .keys()
        .any(|name| !matches!(name.as_str(), "POSITION" | "NORMAL" | "TEXCOORD_0"))
        || !attributes.contains_key("POSITION")
        || !attributes.contains_key("TEXCOORD_0")
    {
        return Err(unsupported(
            "texture projection accepts only POSITION, optional NORMAL, and TEXCOORD_0; other vertex attributes are not transferred",
        ));
    }
    validate_projection_material(&glb.document, snapshot.material)?;
    let binary = glb.binary.as_slice();
    let position_data = read_accessor(&glb.document, binary, snapshot.position_accessor)?;
    require_accessor(
        &position_data,
        "POSITION",
        5126,
        "VEC3",
        snapshot.source_vertices,
    )?;
    let positions = decode_vec3(&position_data.bytes)?;
    let uv_data = read_accessor(&glb.document, binary, snapshot.uv_accessor)?;
    require_accessor(
        &uv_data,
        "TEXCOORD_0",
        5126,
        "VEC2",
        snapshot.source_vertices,
    )?;
    let uv = decode_vec2(&uv_data.bytes)?;
    let index_accessor = snapshot.index_accessor.ok_or_else(|| {
        unsupported("texture projection requires an indexed triangle source mesh")
    })?;
    let indices = read_indices(&glb.document, binary, index_accessor)?;
    if positions.is_empty()
        || positions.len() > 100_000
        || indices.is_empty()
        || indices.len() > 1_500_000
        || !indices.len().is_multiple_of(3)
        || indices
            .iter()
            .any(|index| *index as usize >= positions.len())
    {
        return Err(unsupported(
            "projection source requires 1..100,000 vertices and 1..500,000 valid indexed triangles",
        ));
    }
    Ok(ProjectionSourceMesh {
        positions,
        uv,
        indices,
    })
}

/// Generate a new atlas for remeshed target geometry and project only supported
/// embedded appearance maps from a validated source GLB.
pub fn bake_projected_mesh(
    source_glb: &[u8],
    target_positions: &[[f32; 3]],
    target_normals: &[[f32; 3]],
    target_indices: &[u32],
    options: AtlasBakeOptions,
    max_distance_ratio: f32,
) -> Result<ProjectedMeshBake, AtlasBakeError> {
    validate_options(options)?;
    if target_positions.is_empty()
        || target_positions.len() > MAX_VERTICES
        || target_positions.len() != target_normals.len()
        || target_indices.is_empty()
        || target_indices.len() > MAX_INDICES
        || !target_indices.len().is_multiple_of(3)
        || target_positions.iter().flatten().any(|v| !v.is_finite())
        || target_normals.iter().flatten().any(|v| !v.is_finite())
        || target_indices
            .iter()
            .any(|index| *index as usize >= target_positions.len())
    {
        return Err(invalid(
            "target mesh must have bounded finite position/normal streams and valid indexed triangles",
        ));
    }
    if !max_distance_ratio.is_finite() || !(0.0001..=0.05).contains(&max_distance_ratio) {
        return Err(invalid(
            "projection distance ratio must be between 0.0001 and 0.05",
        ));
    }
    let source = extract_projection_source_mesh(source_glb)?;
    let glb = Glb::parse(source_glb)?;
    let snapshot = validate_supported(&glb.document)?;
    let base_png = read_png_image(&glb.document, &glb.binary, snapshot.base_image)?;
    let mr_png = read_png_image(&glb.document, &glb.binary, snapshot.mr_image)?;
    let material = &glb.document["materials"][snapshot.material];
    let pbr = &material["pbrMetallicRoughness"];
    let base_color_factor = factor_vec4(pbr.get("baseColorFactor"), [1.0; 4], "baseColorFactor")?;
    if (base_color_factor[3] - 1.0).abs() > f32::EPSILON {
        return Err(unsupported(
            "transparent baseColorFactor is not supported by projected rebaking",
        ));
    }
    let metallic_factor = factor_scalar(pbr.get("metallicFactor"), 1.0, "metallicFactor")?;
    let roughness_factor = factor_scalar(pbr.get("roughnessFactor"), 1.0, "roughnessFactor")?;
    let double_sided = material
        .get("doubleSided")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let native = meshharbor_xatlas_sys::unwrap_and_bake_projected(
        meshharbor_xatlas_sys::AtlasInput {
            positions: target_positions,
            normals: Some(target_normals),
            original_uv: &[],
            indices: target_indices,
            base_png: &base_png,
            mr_png: &mr_png,
        },
        meshharbor_xatlas_sys::AtlasSettings {
            resolution: options.resolution,
            padding: options.padding,
        },
        meshharbor_xatlas_sys::ProjectionSource {
            positions: &source.positions,
            uv: &source.uv,
            indices: &source.indices,
        },
        max_distance_ratio,
        0.995,
    )
    .map_err(AtlasBakeError::Native)?;
    if native.xref.is_empty()
        || native.xref.len() != native.uv.len()
        || native.xref.len() > MAX_INDICES
        || native.indices.is_empty()
        || !native.indices.len().is_multiple_of(3)
        || native
            .indices
            .iter()
            .any(|index| *index as usize >= native.xref.len())
        || native
            .xref
            .iter()
            .any(|index| *index as usize >= target_positions.len())
        || native
            .uv
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err(AtlasBakeError::Native(
            "xatlas returned inconsistent projected target geometry".into(),
        ));
    }
    let projection = native.projection.ok_or_else(|| {
        AtlasBakeError::Native("projection worker did not return coverage statistics".into())
    })?;
    let coverage = if projection.attempted_texels == 0 {
        0.0
    } else {
        projection.projected_texels as f32 / projection.attempted_texels as f32
    };
    if coverage < 0.995 || projection.ambiguous_texels > 0 {
        return Err(AtlasBakeError::Unsupported(
            "projected atlas failed its coverage or thin-shell ambiguity gate".into(),
        ));
    }
    let positions = native
        .xref
        .iter()
        .map(|index| target_positions[*index as usize])
        .collect();
    let normals = native
        .xref
        .iter()
        .map(|index| target_normals[*index as usize])
        .collect();
    let report = ProjectedMeshBakeReport {
        source_triangles: (source.indices.len() / 3) as u64,
        target_triangles: (target_indices.len() / 3) as u64,
        output_triangles: (native.indices.len() / 3) as u64,
        output_vertices: native.xref.len() as u64,
        atlas_width: native.width,
        atlas_height: native.height,
        chart_count: native.chart_count,
        covered_texels: native.covered_texels,
        gutter_texels: native.gutter_texels,
        projection_attempted_texels: projection.attempted_texels,
        projection_covered_texels: projection.projected_texels,
        projection_coverage: coverage,
        ambiguous_texels: projection.ambiguous_texels,
        maximum_projection_distance: projection.maximum_distance,
        projection_distance_limit: projection.distance_limit,
        rebaked_maps: vec!["baseColorTexture".into(), "metallicRoughnessTexture".into()],
        warnings: vec![
            "Appearance was projected from the nearest similarly-facing source surface; projection coverage is not a visual-quality certification.".into(),
            "Normal, occlusion, emissive, alpha, and other non-PBR texture maps are not transferred.".into(),
        ],
    };
    Ok(ProjectedMeshBake {
        positions,
        normals,
        uv: native.uv,
        indices: native.indices,
        base_color_png: native.base_color_png,
        metallic_roughness_png: native.metallic_roughness_png,
        base_color_factor,
        metallic_factor,
        roughness_factor,
        double_sided,
        report,
    })
}

/// Encode a projected mesh bake as a standalone, textured triangle-preview GLB.
pub fn encode_projected_mesh_glb(bake: &ProjectedMeshBake) -> Result<Vec<u8>, AtlasBakeError> {
    let vertex_count = bake.positions.len();
    if vertex_count == 0
        || vertex_count != bake.normals.len()
        || vertex_count != bake.uv.len()
        || bake.indices.is_empty()
        || !bake.indices.len().is_multiple_of(3)
        || bake
            .indices
            .iter()
            .any(|index| *index as usize >= vertex_count)
        || bake
            .positions
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        || bake
            .normals
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        || bake
            .uv
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err(invalid(
            "projected mesh streams are inconsistent or out of range",
        ));
    }
    if !bake.base_color_png.starts_with(PNG_MAGIC)
        || !bake.metallic_roughness_png.starts_with(PNG_MAGIC)
        || bake.base_color_png.len() > MAX_PNG_BYTES
        || bake.metallic_roughness_png.len() > MAX_PNG_BYTES
    {
        return Err(invalid(
            "projected output textures must be bounded embedded PNGs",
        ));
    }
    let mut document = json!({
        "asset": { "version": "2.0", "generator": "MeshHarbor projected retopology preview" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0 }],
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }, "indices": 3, "material": 0, "mode": 4 }] }],
        "buffers": [{ "byteLength": 0 }],
        "bufferViews": [],
        "accessors": [],
        "images": [],
        "textures": [{ "source": 0 }, { "source": 1 }],
        "materials": [{
            "pbrMetallicRoughness": {
                "baseColorFactor": bake.base_color_factor,
                "baseColorTexture": { "index": 0, "texCoord": 0 },
                "metallicFactor": bake.metallic_factor,
                "roughnessFactor": bake.roughness_factor,
                "metallicRoughnessTexture": { "index": 1, "texCoord": 0 }
            },
            "doubleSided": bake.double_sided
        }]
    });
    let mut binary = Vec::new();
    let mut position_bytes = Vec::with_capacity(vertex_count * 12);
    let mut normal_bytes = Vec::with_capacity(vertex_count * 12);
    let mut uv_bytes = Vec::with_capacity(vertex_count * 8);
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for (position, normal, uv) in bake
        .positions
        .iter()
        .zip(&bake.normals)
        .zip(&bake.uv)
        .map(|((p, n), uv)| (p, n, uv))
    {
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(position[axis]);
            maximum[axis] = maximum[axis].max(position[axis]);
            position_bytes.extend_from_slice(&position[axis].to_le_bytes());
            normal_bytes.extend_from_slice(&normal[axis].to_le_bytes());
        }
        for component in uv {
            uv_bytes.extend_from_slice(&component.to_le_bytes());
        }
    }
    let position_view = append_view(&mut document, &mut binary, &position_bytes, Some(34962))?;
    let normal_view = append_view(&mut document, &mut binary, &normal_bytes, Some(34962))?;
    let uv_view = append_view(&mut document, &mut binary, &uv_bytes, Some(34962))?;
    let index_bytes: Vec<u8> = bake
        .indices
        .iter()
        .flat_map(|index| index.to_le_bytes())
        .collect();
    let index_view = append_view(&mut document, &mut binary, &index_bytes, Some(34963))?;
    let base_view = append_view(&mut document, &mut binary, &bake.base_color_png, None)?;
    let mr_view = append_view(
        &mut document,
        &mut binary,
        &bake.metallic_roughness_png,
        None,
    )?;
    let position_accessor = append_new_accessor(
        &mut document,
        position_view,
        vertex_count,
        5126,
        "VEC3",
        None,
    )?;
    document["accessors"][position_accessor]["min"] = json!(minimum);
    document["accessors"][position_accessor]["max"] = json!(maximum);
    append_new_accessor(&mut document, normal_view, vertex_count, 5126, "VEC3", None)?;
    append_new_accessor(&mut document, uv_view, vertex_count, 5126, "VEC2", None)?;
    append_new_accessor(
        &mut document,
        index_view,
        bake.indices.len(),
        5125,
        "SCALAR",
        None,
    )?;
    document["images"] = json!([
        { "bufferView": base_view, "mimeType": "image/png" },
        { "bufferView": mr_view, "mimeType": "image/png" }
    ]);
    document["buffers"][0]["byteLength"] = json!(binary.len());
    if binary.len() > 128 * 1024 * 1024 {
        return Err(AtlasBakeError::Output(
            "projected preview exceeds the 128 MiB binary limit".into(),
        ));
    }
    let output = Glb {
        document,
        binary,
        other_chunks: Vec::new(),
    }
    .write()?;
    if output.len() > 128 * 1024 * 1024 {
        return Err(AtlasBakeError::Output(
            "projected preview exceeds the 128 MiB output limit".into(),
        ));
    }
    Ok(output)
}

fn validate_projection_scene(document: &Value) -> Result<(), AtlasBakeError> {
    let nodes = array(document, "nodes")?;
    let scenes = array(document, "scenes")?;
    if nodes.len() != 1
        || nodes[0].get("mesh").and_then(Value::as_u64) != Some(0)
        || nodes[0].get("skin").is_some()
        || nodes[0]
            .get("children")
            .and_then(Value::as_array)
            .is_some_and(|children| !children.is_empty())
        || ["matrix", "translation", "rotation", "scale"]
            .iter()
            .any(|key| nodes[0].get(*key).is_some())
        || scenes.len() != 1
        || array(&scenes[0], "nodes")?.len() != 1
        || array(&scenes[0], "nodes")?[0].as_u64() != Some(0)
        || document.get("scene").and_then(Value::as_u64).unwrap_or(0) != 0
        || document
            .get("cameras")
            .and_then(Value::as_array)
            .is_some_and(|cameras| !cameras.is_empty())
    {
        return Err(unsupported(
            "projection requires one untransformed static mesh node in one scene",
        ));
    }
    Ok(())
}

fn validate_projection_material(
    document: &Value,
    material_index: usize,
) -> Result<(), AtlasBakeError> {
    let material = array(document, "materials")?
        .get(material_index)
        .ok_or_else(|| invalid("material index is out of range"))?;
    if material
        .get("alphaMode")
        .and_then(Value::as_str)
        .unwrap_or("OPAQUE")
        != "OPAQUE"
    {
        return Err(unsupported(
            "MASK and BLEND alpha modes are not supported by projected rebaking",
        ));
    }
    let pbr = material
        .get("pbrMetallicRoughness")
        .ok_or_else(|| unsupported("PBR metallic-roughness material is required"))?;
    let base = factor_vec4(pbr.get("baseColorFactor"), [1.0; 4], "baseColorFactor")?;
    if (base[3] - 1.0).abs() > f32::EPSILON {
        return Err(unsupported(
            "transparent baseColorFactor is not supported by projected rebaking",
        ));
    }
    let emission = factor_vec3(material.get("emissiveFactor"), [0.0; 3], "emissiveFactor")?;
    if emission.iter().any(|value| value.abs() > f32::EPSILON) {
        return Err(unsupported(
            "non-zero emissiveFactor is not transferred by projected rebaking",
        ));
    }
    if pbr
        .get("extensions")
        .and_then(Value::as_object)
        .is_some_and(|extensions| !extensions.is_empty())
    {
        return Err(unsupported(
            "PBR material extensions are not supported by projected rebaking",
        ));
    }
    Ok(())
}

fn factor_scalar(value: Option<&Value>, default: f32, name: &str) -> Result<f32, AtlasBakeError> {
    let Some(value) = value else {
        return Ok(default);
    };
    let value = value
        .as_f64()
        .ok_or_else(|| invalid(format!("{name} must be numeric")))? as f32;
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid(format!(
            "{name} must be a finite value from 0 to 1"
        )));
    }
    Ok(value)
}

fn factor_vec4(
    value: Option<&Value>,
    default: [f32; 4],
    name: &str,
) -> Result<[f32; 4], AtlasBakeError> {
    let Some(value) = value else {
        return Ok(default);
    };
    let values = value
        .as_array()
        .ok_or_else(|| invalid(format!("{name} must be an array")))?;
    if values.len() != 4 {
        return Err(invalid(format!("{name} must have four components")));
    }
    let mut output = [0.0; 4];
    for (index, component) in values.iter().enumerate() {
        let number = component
            .as_f64()
            .ok_or_else(|| invalid(format!("{name} must contain numbers")))?
            as f32;
        if !number.is_finite() || !(0.0..=1.0).contains(&number) {
            return Err(invalid(format!(
                "{name} components must be finite values from 0 to 1"
            )));
        }
        output[index] = number;
    }
    Ok(output)
}

fn factor_vec3(
    value: Option<&Value>,
    default: [f32; 3],
    name: &str,
) -> Result<[f32; 3], AtlasBakeError> {
    let Some(value) = value else {
        return Ok(default);
    };
    let values = value
        .as_array()
        .ok_or_else(|| invalid(format!("{name} must be an array")))?;
    if values.len() != 3 {
        return Err(invalid(format!("{name} must have three components")));
    }
    let mut output = [0.0; 3];
    for (index, component) in values.iter().enumerate() {
        let number = component
            .as_f64()
            .ok_or_else(|| invalid(format!("{name} must contain numbers")))?
            as f32;
        if !number.is_finite() || number < 0.0 {
            return Err(invalid(format!(
                "{name} components must be finite and non-negative"
            )));
        }
        output[index] = number;
    }
    Ok(output)
}

fn validate_options(options: AtlasBakeOptions) -> Result<(), AtlasBakeError> {
    if !(64..=4096).contains(&options.resolution) || options.padding > 64 {
        return Err(AtlasBakeError::InvalidInput(
            "resolution must be 64..4096 and padding must be 0..64".into(),
        ));
    }
    if u64::from(options.resolution) * u64::from(options.resolution) > 16_777_216 {
        return Err(AtlasBakeError::InvalidInput(
            "atlas exceeds the 16M-texel processing limit".into(),
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct SupportedInput {
    position_accessor: usize,
    uv_accessor: usize,
    normal_accessor: Option<usize>,
    index_accessor: Option<usize>,
    source_vertices: usize,
    material: usize,
    base_texture: usize,
    mr_texture: usize,
    base_image: usize,
    mr_image: usize,
}

fn validate_supported(document: &Value) -> Result<SupportedInput, AtlasBakeError> {
    if document
        .get("asset")
        .and_then(|asset| asset.get("version"))
        .and_then(Value::as_str)
        != Some("2.0")
    {
        return Err(unsupported("the asset must use glTF 2.0"));
    }
    if document
        .get("extensionsUsed")
        .and_then(Value::as_array)
        .is_some_and(|extensions| !extensions.is_empty())
        || document
            .get("extensionsRequired")
            .and_then(Value::as_array)
            .is_some_and(|extensions| !extensions.is_empty())
    {
        return Err(AtlasBakeError::Unsupported(
            "GLB extensions are not handled by this initial implementation".into(),
        ));
    }
    if document
        .get("skins")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.is_empty())
        || document
            .get("animations")
            .and_then(Value::as_array)
            .is_some_and(|values| !values.is_empty())
    {
        return Err(AtlasBakeError::Unsupported(
            "animated and skinned assets are not supported".into(),
        ));
    }
    let meshes = array(document, "meshes")?;
    if meshes.len() != 1 {
        return Err(unsupported("exactly one mesh is supported"));
    }
    let primitives = meshes[0]
        .get("primitives")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("mesh primitives are missing"))?;
    if primitives.len() != 1 {
        return Err(unsupported("exactly one primitive is supported"));
    }
    let primitive = &primitives[0];
    if primitive.get("mode").and_then(Value::as_u64).unwrap_or(4) != 4 {
        return Err(unsupported("only TRIANGLES primitives are supported"));
    }
    if primitive.get("targets").is_some()
        || primitive
            .get("extensions")
            .and_then(Value::as_object)
            .is_some_and(|values| !values.is_empty())
    {
        return Err(unsupported(
            "morph targets and primitive extensions are not supported",
        ));
    }
    let attributes = primitive
        .get("attributes")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("primitive attributes are missing"))?;
    let position_accessor = as_index(
        attributes
            .get("POSITION")
            .ok_or_else(|| invalid("POSITION is missing"))?,
        "POSITION accessor",
    )?;
    let uv_accessor = as_index(
        attributes
            .get("TEXCOORD_0")
            .ok_or_else(|| unsupported("an existing TEXCOORD_0 is required for rebaking"))?,
        "TEXCOORD_0 accessor",
    )?;
    if attributes.contains_key("TANGENT") {
        return Err(unsupported(
            "TANGENT must be recomputed after UV changes and is not yet supported",
        ));
    }
    let normal_accessor = attributes
        .get("NORMAL")
        .map(|value| as_index(value, "NORMAL accessor"))
        .transpose()?;
    let position = array(document, "accessors")?
        .get(position_accessor)
        .ok_or_else(|| invalid("POSITION accessor is out of range"))?;
    let source_vertices = as_usize(position.get("count"), "POSITION count")?;
    if source_vertices == 0 || source_vertices > MAX_VERTICES {
        return Err(unsupported(
            "the mesh is empty or exceeds the 10M-vertex safety limit",
        ));
    }
    if array(document, "materials")?.len() != 1 {
        return Err(unsupported("exactly one material is supported"));
    }
    let material = as_index(
        primitive
            .get("material")
            .ok_or_else(|| unsupported("the primitive must reference a PBR material"))?,
        "material index",
    )?;
    if material != 0 {
        return Err(unsupported("the primitive must use material 0"));
    }
    if primitive.get("extensions").is_some() {
        return Err(unsupported("primitive extensions are not supported"));
    }
    let material_value = array(document, "materials")?
        .get(material)
        .ok_or_else(|| invalid("material index is out of range"))?;
    if material_value
        .get("extensions")
        .and_then(Value::as_object)
        .is_some_and(|values| !values.is_empty())
    {
        return Err(unsupported("material extensions are not supported"));
    }
    if material_value.get("normalTexture").is_some()
        || material_value.get("occlusionTexture").is_some()
        || material_value.get("emissiveTexture").is_some()
    {
        return Err(unsupported(
            "normal, occlusion, and emissive maps are not baked; refusing to leave stale UV references",
        ));
    }
    let pbr = material_value
        .get("pbrMetallicRoughness")
        .ok_or_else(|| unsupported("a PBR metallic-roughness material is required"))?;
    let base_info = pbr
        .get("baseColorTexture")
        .ok_or_else(|| unsupported("baseColorTexture is required"))?;
    let mr_info = pbr
        .get("metallicRoughnessTexture")
        .ok_or_else(|| unsupported("metallicRoughnessTexture is required"))?;
    let (base_texture, base_image) = texture_source(document, base_info)?;
    let (mr_texture, mr_image) = texture_source(document, mr_info)?;
    if base_texture == mr_texture {
        return Err(unsupported(
            "base-color and metallic-roughness slots must use separate texture objects",
        ));
    }

    let buffers = array(document, "buffers")?;
    if buffers.len() != 1 || buffers[0].get("uri").is_some() {
        return Err(unsupported("one embedded GLB buffer is required"));
    }
    let index_accessor = primitive
        .get("indices")
        .map(|value| as_index(value, "index accessor"))
        .transpose()?;
    Ok(SupportedInput {
        position_accessor,
        uv_accessor,
        normal_accessor,
        index_accessor,
        source_vertices,
        material,
        base_texture,
        mr_texture,
        base_image,
        mr_image,
    })
}

fn texture_source(document: &Value, info: &Value) -> Result<(usize, usize), AtlasBakeError> {
    if info
        .get("extensions")
        .and_then(Value::as_object)
        .is_some_and(|values| !values.is_empty())
    {
        return Err(unsupported(
            "texture transforms and texture source extensions are not supported",
        ));
    }
    if info.get("texCoord").and_then(Value::as_u64).unwrap_or(0) != 0 {
        return Err(unsupported(
            "only TEXCOORD_0 texture coordinates are supported",
        ));
    }
    let texture_index = as_index(
        info.get("index")
            .ok_or_else(|| invalid("texture info index is missing"))?,
        "texture index",
    )?;
    let texture = array(document, "textures")?
        .get(texture_index)
        .ok_or_else(|| invalid("texture index is out of range"))?;
    if texture
        .get("extensions")
        .and_then(Value::as_object)
        .is_some_and(|values| !values.is_empty())
    {
        return Err(unsupported("texture extensions are not supported"));
    }
    check_sampler(document, texture)?;
    let image_index = as_index(
        texture
            .get("source")
            .ok_or_else(|| unsupported("extension-only texture sources are not supported"))?,
        "image source",
    )?;
    let image = array(document, "images")?
        .get(image_index)
        .ok_or_else(|| invalid("image source is out of range"))?;
    if image.get("uri").is_some()
        || image.get("bufferView").is_none()
        || image.get("mimeType").and_then(Value::as_str) != Some("image/png")
    {
        return Err(unsupported("only embedded PNG images are supported"));
    }
    Ok((texture_index, image_index))
}

fn check_sampler(document: &Value, texture: &Value) -> Result<(), AtlasBakeError> {
    let Some(sampler_index) = texture.get("sampler") else {
        return Ok(());
    };
    let sampler_index = as_index(sampler_index, "sampler index")?;
    let sampler = array(document, "samplers")?
        .get(sampler_index)
        .ok_or_else(|| invalid("sampler index is out of range"))?;
    let wrap_s = sampler
        .get("wrapS")
        .and_then(Value::as_u64)
        .unwrap_or(10497);
    let wrap_t = sampler
        .get("wrapT")
        .and_then(Value::as_u64)
        .unwrap_or(10497);
    let mag = sampler
        .get("magFilter")
        .and_then(Value::as_u64)
        .unwrap_or(9729);
    let min = sampler
        .get("minFilter")
        .and_then(Value::as_u64)
        .unwrap_or(9987);
    if wrap_s != 10497 || wrap_t != 10497 {
        return Err(unsupported("only REPEAT texture wrapping is supported"));
    }
    if mag != 9729 || !matches!(min, 9729 | 9985 | 9987) {
        return Err(unsupported(
            "only linearly filtered texture samplers are supported",
        ));
    }
    Ok(())
}

fn read_png_image(
    document: &Value,
    bin: &[u8],
    image_index: usize,
) -> Result<Vec<u8>, AtlasBakeError> {
    let image = array(document, "images")?
        .get(image_index)
        .ok_or_else(|| invalid("image index is out of range"))?;
    let view_index = as_index(
        image
            .get("bufferView")
            .ok_or_else(|| invalid("embedded PNG bufferView is missing"))?,
        "image bufferView",
    )?;
    let (start, length, buffer) = view_range(document, bin, view_index)?;
    if buffer != 0 {
        return Err(unsupported(
            "embedded images must reference the first GLB buffer",
        ));
    }
    let data = bin
        .get(start..start + length)
        .ok_or_else(|| invalid("embedded PNG exceeds the GLB binary chunk"))?;
    if !data.starts_with(PNG_MAGIC) {
        return Err(invalid("image claims to be PNG but has no PNG signature"));
    }
    if data.len() > MAX_PNG_BYTES {
        return Err(unsupported("embedded PNG exceeds the 128 MiB safety limit"));
    }
    Ok(data.to_vec())
}

#[derive(Debug)]
struct AccessorData {
    count: usize,
    element_size: usize,
    component_type: u64,
    type_name: String,
    normalized: bool,
    bytes: Vec<u8>,
}

fn read_accessor(
    document: &Value,
    bin: &[u8],
    accessor_index: usize,
) -> Result<AccessorData, AtlasBakeError> {
    let accessor = array(document, "accessors")?
        .get(accessor_index)
        .ok_or_else(|| invalid("accessor index is out of range"))?;
    if accessor.get("sparse").is_some() {
        return Err(unsupported(
            "sparse accessors are not supported by atlas baking",
        ));
    }
    let view_index = as_index(
        accessor
            .get("bufferView")
            .ok_or_else(|| unsupported("accessors without a bufferView are not supported"))?,
        "accessor bufferView",
    )?;
    let (view_start, view_length, buffer) = view_range(document, bin, view_index)?;
    if buffer != 0 {
        return Err(unsupported(
            "all mesh accessors must use the first GLB buffer",
        ));
    }
    let views = array(document, "bufferViews")?;
    let view = &views[view_index];
    let component_type = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("accessor componentType is invalid"))?;
    let component_size = match component_type {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        _ => return Err(unsupported("accessor component type is unsupported")),
    };
    let type_name = accessor
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("accessor type is missing"))?
        .to_owned();
    let component_count = match type_name.as_str() {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        _ => {
            return Err(unsupported(
                "only scalar and vector accessors are supported",
            ));
        }
    };
    let element_size = component_size * component_count;
    let count = as_usize(accessor.get("count"), "accessor count")?;
    if count > MAX_INDICES {
        return Err(unsupported("accessor exceeds the 15M-element safety limit"));
    }
    let accessor_offset = optional_usize(accessor.get("byteOffset"), 0, "accessor byteOffset")?;
    let stride = if let Some(stride) = view.get("byteStride") {
        as_usize(Some(stride), "bufferView byteStride")?
    } else {
        element_size
    };
    if stride < element_size || stride % component_size != 0 {
        return Err(invalid("accessor stride is invalid"));
    }
    let end_in_view = if count == 0 {
        accessor_offset
    } else {
        accessor_offset
            .checked_add(
                (count - 1)
                    .checked_mul(stride)
                    .ok_or_else(|| invalid("accessor size overflow"))?,
            )
            .and_then(|value| value.checked_add(element_size))
            .ok_or_else(|| invalid("accessor size overflow"))?
    };
    if end_in_view > view_length {
        return Err(invalid("accessor exceeds its bufferView"));
    }
    let mut tight = Vec::with_capacity(
        count
            .checked_mul(element_size)
            .ok_or_else(|| invalid("accessor size overflow"))?,
    );
    for item in 0..count {
        let start = view_start + accessor_offset + item * stride;
        tight.extend_from_slice(&bin[start..start + element_size]);
    }
    let normalized = accessor
        .get("normalized")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(AccessorData {
        count,
        element_size,
        component_type,
        type_name,
        normalized,
        bytes: tight,
    })
}

fn view_range(
    document: &Value,
    bin: &[u8],
    view_index: usize,
) -> Result<(usize, usize, usize), AtlasBakeError> {
    let view = array(document, "bufferViews")?
        .get(view_index)
        .ok_or_else(|| invalid("bufferView index is out of range"))?;
    let buffer = as_index(
        view.get("buffer")
            .ok_or_else(|| invalid("bufferView buffer index is missing"))?,
        "bufferView buffer",
    )?;
    let offset = optional_usize(view.get("byteOffset"), 0, "bufferView byteOffset")?;
    let length = as_usize(view.get("byteLength"), "bufferView byteLength")?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| invalid("bufferView range overflow"))?;
    let buffers = array(document, "buffers")?;
    let buffer_length = buffers
        .get(buffer)
        .ok_or_else(|| invalid("bufferView references a missing buffer"))?;
    let buffer_length = as_usize(buffer_length.get("byteLength"), "buffer byteLength")?;
    if end > buffer_length || end > bin.len() {
        return Err(invalid("bufferView exceeds its declared GLB buffer"));
    }
    Ok((offset, length, buffer))
}

fn read_indices(
    document: &Value,
    bin: &[u8],
    accessor_index: usize,
) -> Result<Vec<u32>, AtlasBakeError> {
    let data = read_accessor(document, bin, accessor_index)?;
    if data.type_name != "SCALAR"
        || data.normalized
        || !matches!(data.component_type, 5121 | 5123 | 5125)
    {
        return Err(unsupported(
            "indices must be unsigned 8-, 16-, or 32-bit scalars",
        ));
    }
    let mut result = Vec::with_capacity(data.count);
    match data.component_type {
        5121 => result.extend(data.bytes.iter().map(|value| u32::from(*value))),
        5123 => result.extend(
            data.bytes
                .chunks_exact(2)
                .map(|bytes| u32::from(u16::from_le_bytes([bytes[0], bytes[1]]))),
        ),
        5125 => result.extend(
            data.bytes
                .chunks_exact(4)
                .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        ),
        _ => unreachable!(),
    }
    Ok(result)
}

fn require_accessor(
    data: &AccessorData,
    semantic: &str,
    component_type: u64,
    type_name: &str,
    count: usize,
) -> Result<(), AtlasBakeError> {
    if data.component_type != component_type
        || data.type_name != type_name
        || data.count != count
        || data.normalized
    {
        return Err(unsupported(format!(
            "{semantic} must be a {type_name} float accessor with {count} entries"
        )));
    }
    Ok(())
}

fn decode_vec2(bytes: &[u8]) -> Result<Vec<[f32; 2]>, AtlasBakeError> {
    let mut values = Vec::with_capacity(bytes.len() / 8);
    for pair in bytes.chunks_exact(8) {
        let value = [
            f32::from_le_bytes(pair[0..4].try_into().unwrap()),
            f32::from_le_bytes(pair[4..8].try_into().unwrap()),
        ];
        if value.iter().any(|component| !component.is_finite()) {
            return Err(invalid("UV contains a non-finite number"));
        }
        values.push(value);
    }
    Ok(values)
}

fn decode_vec3(bytes: &[u8]) -> Result<Vec<[f32; 3]>, AtlasBakeError> {
    let mut values = Vec::with_capacity(bytes.len() / 12);
    for triple in bytes.chunks_exact(12) {
        let value = [
            f32::from_le_bytes(triple[0..4].try_into().unwrap()),
            f32::from_le_bytes(triple[4..8].try_into().unwrap()),
            f32::from_le_bytes(triple[8..12].try_into().unwrap()),
        ];
        if value.iter().any(|component| !component.is_finite()) {
            return Err(invalid("POSITION or NORMAL contains a non-finite number"));
        }
        values.push(value);
    }
    Ok(values)
}

fn append_view(
    document: &mut Value,
    bin: &mut Vec<u8>,
    bytes: &[u8],
    target: Option<u64>,
) -> Result<usize, AtlasBakeError> {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let offset = bin.len();
    bin.extend_from_slice(bytes);
    let mut view = Map::new();
    view.insert("buffer".into(), json!(0));
    view.insert("byteOffset".into(), json!(offset));
    view.insert("byteLength".into(), json!(bytes.len()));
    if let Some(target) = target {
        view.insert("target".into(), json!(target));
    }
    let views = document
        .get_mut("bufferViews")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("bufferViews must be an array"))?;
    let index = views.len();
    views.push(Value::Object(view));
    Ok(index)
}

fn append_accessor_like(
    document: &mut Value,
    original: usize,
    view: usize,
    count: usize,
) -> Result<usize, AtlasBakeError> {
    let mut accessor = array(document, "accessors")?
        .get(original)
        .ok_or_else(|| invalid("source accessor index is out of range"))?
        .as_object()
        .ok_or_else(|| invalid("accessor must be an object"))?
        .clone();
    accessor.insert("bufferView".into(), json!(view));
    accessor.insert("byteOffset".into(), json!(0));
    accessor.insert("count".into(), json!(count));
    accessor.remove("min");
    accessor.remove("max");
    let accessors = document
        .get_mut("accessors")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("accessors must be an array"))?;
    let index = accessors.len();
    accessors.push(Value::Object(accessor));
    Ok(index)
}

fn append_new_accessor(
    document: &mut Value,
    view: usize,
    count: usize,
    component_type: u64,
    type_name: &str,
    _bounds: Option<(Vec<f64>, Vec<f64>)>,
) -> Result<usize, AtlasBakeError> {
    let accessor = json!({ "bufferView": view, "byteOffset": 0, "componentType": component_type, "count": count, "type": type_name });
    let accessors = document
        .get_mut("accessors")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("accessors must be an array"))?;
    let index = accessors.len();
    accessors.push(accessor);
    Ok(index)
}

fn append_atlas_texture(
    document: &mut Value,
    texture_index: usize,
    image_index: usize,
    view_index: usize,
) -> Result<usize, AtlasBakeError> {
    let image = array(document, "images")?
        .get(image_index)
        .ok_or_else(|| invalid("source image index is out of range"))?
        .as_object()
        .ok_or_else(|| invalid("image must be an object"))?
        .clone();
    let mut new_image = image;
    new_image.remove("uri");
    new_image.insert("bufferView".into(), json!(view_index));
    new_image.insert("mimeType".into(), json!("image/png"));
    let images = document
        .get_mut("images")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("images must be an array"))?;
    let new_image_index = images.len();
    images.push(Value::Object(new_image));

    let texture = array(document, "textures")?
        .get(texture_index)
        .ok_or_else(|| invalid("source texture index is out of range"))?
        .as_object()
        .ok_or_else(|| invalid("texture must be an object"))?
        .clone();
    let mut new_texture = texture;
    new_texture.insert("source".into(), json!(new_image_index));
    let textures = document
        .get_mut("textures")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("textures must be an array"))?;
    let new_texture_index = textures.len();
    textures.push(Value::Object(new_texture));
    Ok(new_texture_index)
}

fn array<'a>(document: &'a Value, name: &str) -> Result<&'a [Value], AtlasBakeError> {
    document
        .get(name)
        .and_then(Value::as_array)
        .map(|values| values.as_slice())
        .ok_or_else(|| invalid(format!("{name} must be an array")))
}
fn as_index(value: &Value, name: &str) -> Result<usize, AtlasBakeError> {
    let number = value
        .as_u64()
        .ok_or_else(|| invalid(format!("{name} must be an unsigned integer")))?;
    usize::try_from(number).map_err(|_| invalid(format!("{name} is too large")))
}
fn as_usize(value: Option<&Value>, name: &str) -> Result<usize, AtlasBakeError> {
    let number = value
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid(format!("{name} must be an unsigned integer")))?;
    usize::try_from(number).map_err(|_| invalid(format!("{name} is too large")))
}
fn optional_usize(
    value: Option<&Value>,
    default: usize,
    name: &str,
) -> Result<usize, AtlasBakeError> {
    match value {
        Some(value) => as_usize(Some(value), name),
        None => Ok(default),
    }
}
fn invalid(message: impl Into<String>) -> AtlasBakeError {
    AtlasBakeError::InvalidInput(message.into())
}
fn unsupported(message: impl Into<String>) -> AtlasBakeError {
    AtlasBakeError::Unsupported(message.into())
}

#[derive(Debug)]
struct Glb {
    document: Value,
    binary: Vec<u8>,
    other_chunks: Vec<(u32, Vec<u8>)>,
}

impl Glb {
    fn parse(bytes: &[u8]) -> Result<Self, AtlasBakeError> {
        if bytes.len() < 20 || bytes.get(..4) != Some(GLB_MAGIC.as_slice()) {
            return Err(invalid("GLB header is missing or truncated"));
        }
        let version = read_u32(bytes, 4)?;
        let declared_length = usize::try_from(read_u32(bytes, 8)?)
            .map_err(|_| invalid("GLB length is not representable"))?;
        if version != 2 || declared_length != bytes.len() {
            return Err(unsupported(
                "only complete glTF binary version 2 files are supported",
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
                .ok_or_else(|| invalid("GLB chunk header is truncated"))?;
            let length = usize::try_from(read_u32(bytes, cursor)?)
                .map_err(|_| invalid("GLB chunk length is invalid"))?;
            let kind = read_u32(bytes, cursor + 4)?;
            if !length.is_multiple_of(4) {
                return Err(invalid("GLB chunk length must be 4-byte aligned"));
            }
            let start = header_end;
            let end = start
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| invalid("GLB chunk exceeds file bounds"))?;
            if chunk_index == 0 && kind != JSON_CHUNK {
                return Err(invalid("GLB must begin with a JSON chunk"));
            }
            match kind {
                JSON_CHUNK if document.is_none() && chunk_index == 0 => {
                    document = Some(
                        serde_json::from_slice(&bytes[start..end])
                            .map_err(|_| invalid("GLB JSON chunk is invalid"))?,
                    );
                }
                JSON_CHUNK => return Err(invalid("GLB contains more than one JSON chunk")),
                BIN_CHUNK if binary.is_none() => binary = Some(bytes[start..end].to_vec()),
                BIN_CHUNK => return Err(invalid("GLB contains more than one BIN chunk")),
                unknown => other_chunks.push((unknown, bytes[start..end].to_vec())),
            }
            cursor = end;
            chunk_index += 1;
        }
        let document = document.ok_or_else(|| invalid("GLB JSON chunk is missing"))?;
        let binary = binary.ok_or_else(|| unsupported("GLB has no embedded BIN chunk"))?;
        let buffers = array(&document, "buffers")?;
        if buffers.len() != 1 {
            return Err(unsupported("exactly one embedded buffer is required"));
        }
        let declared_bin_length = as_usize(buffers[0].get("byteLength"), "buffer byteLength")?;
        if declared_bin_length > binary.len() || binary.len() - declared_bin_length > 3 {
            return Err(invalid(
                "GLB BIN chunk length does not match its buffer declaration",
            ));
        }
        Ok(Self {
            document,
            binary,
            other_chunks,
        })
    }

    fn write(&self) -> Result<Vec<u8>, AtlasBakeError> {
        let mut chunks = Vec::<(u32, Vec<u8>)>::new();
        let mut json_bytes = serde_json::to_vec(&self.document)
            .map_err(|error| AtlasBakeError::Output(error.to_string()))?;
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        chunks.push((JSON_CHUNK, json_bytes));
        let mut binary = self.binary.clone();
        while !binary.len().is_multiple_of(4) {
            binary.push(0);
        }
        chunks.push((BIN_CHUNK, binary));
        chunks.extend(self.other_chunks.iter().cloned());
        let total = chunks
            .iter()
            .try_fold(12usize, |size, (_, chunk)| {
                size.checked_add(8)?.checked_add(chunk.len())
            })
            .ok_or_else(|| AtlasBakeError::Output("GLB size overflow".into()))?;
        let total_u32 = u32::try_from(total)
            .map_err(|_| AtlasBakeError::Output("GLB exceeds the 4 GiB format limit".into()))?;
        let mut output = Vec::with_capacity(total);
        output.extend_from_slice(GLB_MAGIC);
        output.extend_from_slice(&2u32.to_le_bytes());
        output.extend_from_slice(&total_u32.to_le_bytes());
        for (kind, chunk) in chunks {
            let length = u32::try_from(chunk.len()).map_err(|_| {
                AtlasBakeError::Output("GLB chunk exceeds the 4 GiB format limit".into())
            })?;
            output.extend_from_slice(&length.to_le_bytes());
            output.extend_from_slice(&kind.to_le_bytes());
            output.extend_from_slice(&chunk);
        }
        Ok(output)
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, AtlasBakeError> {
    let end = offset
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| invalid("GLB integer is truncated"))?;
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| invalid("GLB integer is truncated"))?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

#[cfg(test)]
mod tests;
