//! Strict QuadriFlow OBJ validation, quad preservation, and optional limited texture projection.
//!
//! GLB can only store triangle primitives. Keep the QuadriFlow OBJ as the editable
//! quad-topology artifact and use the GLB produced here only as a viewer preview.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as FmtWrite,
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use meshharbor_atlas_baker::{
    AtlasBakeOptions, ProjectedMeshBakeReport, bake_projected_mesh, encode_projected_mesh_glb,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    CoreError, CoreResult, RemeshRequest, RetopologyOptions, glb::extract_retopology_geometry,
    validate_remesh_request,
};

const MAX_OBJ_BYTES: usize = 64 * 1024 * 1024;
const MAX_OBJ_LINE_BYTES: usize = 4 * 1024;
const MAX_RECORDS: usize = 751_000;
const MAX_VERTICES: usize = 250_000;
const MAX_QUADS: usize = 500_000;
const MAX_PREVIEW_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct QuadMesh {
    positions: Vec<[f32; 3]>,
    quads: Vec<[u32; 4]>,
}

impl QuadMesh {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn quad_count(&self) -> usize {
        self.quads.len()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QuadMeshReport {
    pub output_vertices: u64,
    pub output_quads: u64,
    pub preview_triangles: u64,
    pub warnings: Vec<String>,
}

struct QuadPreviewGeometry {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

/// Parses only the geometry-only OBJ emitted by the pinned QuadriFlow CLI.
///
/// It rejects materials, UVs, normals, groups, negative indices, non-quad faces,
/// open/non-manifold topology, degenerate faces, and outputs outside a broad
/// source-bounds envelope. It does not certify self-intersection or surface error.
pub fn parse_quad_obj_output(
    bytes: &[u8],
    source_positions: &[[f32; 3]],
) -> CoreResult<(QuadMesh, QuadMeshReport)> {
    if bytes.is_empty() || bytes.len() > MAX_OBJ_BYTES {
        return Err(output_error(
            "QuadriFlow OBJ output is empty or exceeds 64 MiB.",
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| output_error("QuadriFlow OBJ output is not UTF-8."))?;
    let source_bounds = bounds(source_positions)
        .map_err(|_| output_error("Source mesh bounds are invalid for QuadriFlow output."))?;

    let mut positions = Vec::new();
    let mut quads = Vec::new();
    let mut record_count = 0usize;

    for (line_number, raw_line) in text.lines().enumerate() {
        record_count += 1;
        if record_count > MAX_RECORDS {
            return Err(output_error("QuadriFlow OBJ output has too many records."));
        }
        if raw_line.len() > MAX_OBJ_LINE_BYTES {
            return Err(output_error(format!(
                "QuadriFlow OBJ line {} exceeds the record-size limit.",
                line_number + 1
            )));
        }
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut fields = line.split_whitespace();
        match fields.next().unwrap_or_default() {
            "v" => {
                if positions.len() >= MAX_VERTICES {
                    return Err(output_error("QuadriFlow output exceeds 250,000 vertices."));
                }
                let x = parse_float(fields.next(), line_number + 1)?;
                let y = parse_float(fields.next(), line_number + 1)?;
                let z = parse_float(fields.next(), line_number + 1)?;
                if fields.next().is_some() {
                    return Err(output_error(format!(
                        "QuadriFlow OBJ vertex on line {} must have exactly three coordinates.",
                        line_number + 1
                    )));
                }
                positions.push([x, y, z]);
            }
            "f" => {
                if quads.len() >= MAX_QUADS {
                    return Err(output_error(
                        "QuadriFlow output exceeds 500,000 quad faces.",
                    ));
                }
                let mut face = [0u32; 4];
                for index in &mut face {
                    let token = fields.next().ok_or_else(|| {
                        output_error(format!(
                            "QuadriFlow OBJ face on line {} must have four vertices.",
                            line_number + 1
                        ))
                    })?;
                    let one_based = token.parse::<u32>().map_err(|_| {
                        output_error(format!(
                            "QuadriFlow OBJ face on line {} contains an unsupported index.",
                            line_number + 1
                        ))
                    })?;
                    if one_based == 0 {
                        return Err(output_error(format!(
                            "QuadriFlow OBJ face on line {} contains a zero index.",
                            line_number + 1
                        )));
                    }
                    *index = one_based - 1;
                }
                if fields.next().is_some() {
                    return Err(output_error(format!(
                        "QuadriFlow OBJ face on line {} must contain exactly four indices.",
                        line_number + 1
                    )));
                }
                quads.push(face);
            }
            _ => {
                return Err(output_error(format!(
                    "QuadriFlow OBJ contains an unsupported record on line {}.",
                    line_number + 1
                )));
            }
        }
    }

    let mesh = QuadMesh { positions, quads };
    validate_quad_mesh(&mesh, source_bounds)?;
    let report = QuadMeshReport {
        output_vertices: mesh.positions.len() as u64,
        output_quads: mesh.quads.len() as u64,
        preview_triangles: (mesh.quads.len() * 2) as u64,
        warnings: vec![
            "Keep the QuadriFlow OBJ artifact to preserve editable quad topology; the GLB is a triangulated viewer preview.".into(),
            "This validator does not test self-intersections or certify a geometric-error tolerance.".into(),
        ],
    };
    Ok((mesh, report))
}

/// Creates a triangle-only GLB preview from a validated QuadMesh.
///
/// This does not replace the original OBJ artifact: glTF/GLB has no quad-face
/// primitive mode, so the editable quad topology would otherwise be lost.
pub fn quad_mesh_preview_glb(mesh: &QuadMesh) -> CoreResult<Vec<u8>> {
    let QuadPreviewGeometry {
        positions,
        normals: packed_normals,
        indices: triangle_indices,
    } = quad_mesh_preview_geometry(mesh)?;

    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for position in &positions {
        for (axis, component) in position.iter().enumerate() {
            minimum[axis] = minimum[axis].min(*component);
            maximum[axis] = maximum[axis].max(*component);
        }
    }

    let position_byte_length = positions.len() * 12;
    let normal_byte_offset = position_byte_length;
    let normal_byte_length = packed_normals.len() * 12;
    let index_byte_offset = normal_byte_offset + normal_byte_length;
    let index_byte_length = triangle_indices.len() * 4;
    let mut binary = Vec::with_capacity(index_byte_offset + index_byte_length);

    for position in &positions {
        for component in position {
            binary.extend_from_slice(&component.to_le_bytes());
        }
    }
    for normal in &packed_normals {
        for component in normal {
            binary.extend_from_slice(&component.to_le_bytes());
        }
    }
    for index in &triangle_indices {
        binary.extend_from_slice(&index.to_le_bytes());
    }

    let document = json!({
        "asset": { "version": "2.0", "generator": "MeshHarbor QuadriFlow preview" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0 }],
        "meshes": [{
            "primitives": [{
                "attributes": { "POSITION": 0, "NORMAL": 1 },
                "indices": 2,
                "mode": 4
            }]
        }],
        "buffers": [{ "byteLength": binary.len() }],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": position_byte_length, "target": 34962 },
            { "buffer": 0, "byteOffset": normal_byte_offset, "byteLength": normal_byte_length, "target": 34962 },
            { "buffer": 0, "byteOffset": index_byte_offset, "byteLength": index_byte_length, "target": 34963 }
        ],
        "accessors": [
            {
                "bufferView": 0,
                "byteOffset": 0,
                "componentType": 5126,
                "count": positions.len(),
                "type": "VEC3",
                "min": minimum,
                "max": maximum
            },
            {
                "bufferView": 1,
                "byteOffset": 0,
                "componentType": 5126,
                "count": packed_normals.len(),
                "type": "VEC3"
            },
            {
                "bufferView": 2,
                "byteOffset": 0,
                "componentType": 5125,
                "count": triangle_indices.len(),
                "type": "SCALAR",
                "min": [0],
                "max": [positions.len() - 1]
            }
        ]
    });
    let mut json_chunk = serde_json::to_vec(&document)
        .map_err(|error| output_error(format!("Could not encode quad preview GLB: {error}")))?;
    while !json_chunk.len().is_multiple_of(4) {
        json_chunk.push(b' ');
    }
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }

    let total_length = 12usize + 8 + json_chunk.len() + 8 + binary.len();
    if total_length > MAX_PREVIEW_BYTES {
        return Err(output_error("Quad preview GLB exceeds 128 MiB."));
    }
    let total_length =
        u32::try_from(total_length).map_err(|_| output_error("Quad preview GLB is too large."))?;
    let mut glb = Vec::with_capacity(total_length as usize);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&total_length.to_le_bytes());
    glb.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
    glb.extend_from_slice(&json_chunk);
    glb.extend_from_slice(&(binary.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x004E4942u32.to_le_bytes());
    glb.extend_from_slice(&binary);
    Ok(glb)
}

fn quad_mesh_preview_geometry(mesh: &QuadMesh) -> CoreResult<QuadPreviewGeometry> {
    let mesh_bounds = bounds(&mesh.positions)
        .map_err(|_| output_error("Quad preview has invalid mesh bounds."))?;
    let mut triangle_indices = Vec::with_capacity(mesh.quads.len() * 6);
    for quad in &mesh.quads {
        triangle_indices.extend(triangulate_quad(
            quad,
            &mesh.positions,
            mesh_bounds.diagonal,
        )?);
    }
    let mut accumulated = vec![[0.0f64; 3]; mesh.positions.len()];
    for triangle in triangle_indices.chunks_exact(3) {
        let a = point(mesh.positions[triangle[0] as usize]);
        let b = point(mesh.positions[triangle[1] as usize]);
        let c = point(mesh.positions[triangle[2] as usize]);
        let normal = cross(sub(b, a), sub(c, a));
        for index in triangle {
            for axis in 0..3 {
                accumulated[*index as usize][axis] += normal[axis];
            }
        }
    }
    let mut normals = Vec::with_capacity(mesh.positions.len());
    for normal in accumulated {
        let length = dot(normal, normal).sqrt();
        if !length.is_finite() || length <= 1.0e-18 {
            return Err(output_error(
                "Quad preview contains a vertex with an undefined normal.",
            ));
        }
        normals.push([
            (normal[0] / length) as f32,
            (normal[1] / length) as f32,
            (normal[2] / length) as f32,
        ]);
    }
    Ok(QuadPreviewGeometry {
        positions: mesh.positions.clone(),
        normals,
        indices: triangle_indices,
    })
}

#[derive(Clone, Copy)]
struct Bounds {
    center: [f64; 3],
    diagonal: f64,
}

fn validate_quad_mesh(mesh: &QuadMesh, source_bounds: Bounds) -> CoreResult<()> {
    if mesh.positions.is_empty()
        || mesh.positions.len() > MAX_VERTICES
        || mesh.quads.is_empty()
        || mesh.quads.len() > MAX_QUADS
    {
        return Err(output_error(
            "QuadriFlow returned an empty or oversized quad mesh.",
        ));
    }
    for position in &mesh.positions {
        if position.iter().any(|component| !component.is_finite()) {
            return Err(output_error("QuadriFlow returned non-finite positions."));
        }
        let relative = [
            position[0] as f64 - source_bounds.center[0],
            position[1] as f64 - source_bounds.center[1],
            position[2] as f64 - source_bounds.center[2],
        ];
        if dot(relative, relative) > (2.0 * source_bounds.diagonal).powi(2) {
            return Err(output_error(
                "QuadriFlow output exceeded the broad 2x source-bounds-diagonal envelope.",
            ));
        }
    }

    let mesh_bounds = bounds(&mesh.positions)
        .map_err(|_| output_error("QuadriFlow returned degenerate output bounds."))?;
    let mut edge_faces = HashMap::<(u32, u32), (u8, i8)>::new();
    let mut unique_faces = HashSet::<[u32; 4]>::new();
    for quad in &mesh.quads {
        if quad
            .iter()
            .any(|index| *index as usize >= mesh.positions.len())
        {
            return Err(output_error(
                "QuadriFlow returned an out-of-range quad index.",
            ));
        }
        if (0..4).any(|index| quad[index] == quad[(index + 1) % 4]) {
            return Err(output_error("QuadriFlow returned a degenerate quad."));
        }
        let mut face_key = *quad;
        face_key.sort_unstable();
        if !unique_faces.insert(face_key) {
            return Err(output_error("QuadriFlow returned duplicate quad faces."));
        }
        triangulate_quad(quad, &mesh.positions, mesh_bounds.diagonal)?;

        for edge in 0..4 {
            let u = quad[edge];
            let v = quad[(edge + 1) % 4];
            let key = if u < v { (u, v) } else { (v, u) };
            let (count, winding) = edge_faces.entry(key).or_default();
            *count = count.saturating_add(1);
            *winding += if u < v { 1 } else { -1 };
            if *count > 2 {
                return Err(output_error("QuadriFlow returned a non-manifold edge."));
            }
        }
    }

    if edge_faces.values().any(|(count, _)| *count != 2) {
        return Err(output_error(
            "QuadriFlow returned an open or non-watertight quad mesh.",
        ));
    }
    if edge_faces.values().any(|(_, winding)| *winding != 0) {
        return Err(output_error(
            "QuadriFlow returned inconsistently oriented quad faces.",
        ));
    }
    if !closed_quad_vertex_links_are_manifold(&mesh.quads, mesh.positions.len()) {
        return Err(output_error(
            "QuadriFlow returned a vertex-non-manifold quad mesh.",
        ));
    }
    Ok(())
}

fn closed_quad_vertex_links_are_manifold(quads: &[[u32; 4]], vertex_count: usize) -> bool {
    let mut links = vec![Vec::<(u32, u32)>::new(); vertex_count];
    for [a, b, c, d] in quads {
        links[*a as usize].extend([(*d, *b), (*b, *d)]);
        links[*b as usize].extend([(*a, *c), (*c, *a)]);
        links[*c as usize].extend([(*b, *d), (*d, *b)]);
        links[*d as usize].extend([(*c, *a), (*a, *c)]);
    }

    for link in &mut links {
        if link.len() < 6 || !link.len().is_multiple_of(2) {
            return false;
        }
        link.sort_unstable();

        let mut unique_neighbors = 0usize;
        let mut position = 0usize;
        while position < link.len() {
            let neighbor = link[position].0;
            let start = position;
            while position < link.len() && link[position].0 == neighbor {
                position += 1;
            }
            if position - start != 2 {
                return false;
            }
            unique_neighbors += 1;
        }

        let start = link[0].0;
        let mut previous = u32::MAX;
        let mut current = start;
        let mut visited = 0usize;
        loop {
            if visited >= unique_neighbors {
                return false;
            }
            let offset = link.partition_point(|(neighbor, _)| *neighbor < current);
            if offset + 1 >= link.len() || link[offset].0 != current {
                return false;
            }
            let next = if link[offset].1 != previous {
                link[offset].1
            } else {
                link[offset + 1].1
            };
            previous = current;
            current = next;
            visited += 1;
            if current == start {
                break;
            }
        }
        if visited != unique_neighbors {
            return false;
        }
    }
    true
}

fn triangulate_quad(
    quad: &[u32; 4],
    positions: &[[f32; 3]],
    diagonal: f64,
) -> CoreResult<[u32; 6]> {
    let [a, b, c, d] = *quad;
    let first = [a, b, c, a, c, d];
    let second = [a, b, d, b, c, d];
    let first_valid = valid_triangulation(&first, positions, diagonal);
    let second_valid = valid_triangulation(&second, positions, diagonal);

    match (first_valid, second_valid) {
        (true, true) => {
            let diagonal_ac = squared_distance(positions[a as usize], positions[c as usize]);
            let diagonal_bd = squared_distance(positions[b as usize], positions[d as usize]);
            Ok(if diagonal_ac <= diagonal_bd {
                first
            } else {
                second
            })
        }
        (true, false) => Ok(first),
        (false, true) => Ok(second),
        (false, false) => Err(output_error(
            "QuadriFlow returned a zero-area, folded, or self-crossing quad.",
        )),
    }
}

fn valid_triangulation(triangles: &[u32; 6], positions: &[[f32; 3]], diagonal: f64) -> bool {
    let [a, b, c, d, e, f] = *triangles;
    let first = normalized_triangle_normal(
        positions[a as usize],
        positions[b as usize],
        positions[c as usize],
        diagonal,
    );
    let second = normalized_triangle_normal(
        positions[d as usize],
        positions[e as usize],
        positions[f as usize],
        diagonal,
    );
    let first_length = dot(first, first);
    let second_length = dot(second, second);
    first_length.is_finite()
        && second_length.is_finite()
        && first_length > 1.0e-20
        && second_length > 1.0e-20
        && dot(first, second) > 0.0
}

fn normalized_triangle_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3], diagonal: f64) -> [f64; 3] {
    let a = point(a);
    let b = point(b);
    let c = point(c);
    let edge_a = scale(sub(b, a), 1.0 / diagonal);
    let edge_b = scale(sub(c, a), 1.0 / diagonal);
    cross(edge_a, edge_b)
}

fn squared_distance(a: [f32; 3], b: [f32; 3]) -> f64 {
    let difference = sub(point(a), point(b));
    dot(difference, difference)
}

fn parse_float(token: Option<&str>, line_number: usize) -> CoreResult<f32> {
    let value = token
        .ok_or_else(|| {
            output_error(format!(
                "QuadriFlow OBJ vertex on line {line_number} is incomplete."
            ))
        })?
        .parse::<f32>()
        .map_err(|_| {
            output_error(format!(
                "QuadriFlow OBJ vertex on line {line_number} contains an invalid coordinate."
            ))
        })?;
    if !value.is_finite() {
        return Err(output_error(format!(
            "QuadriFlow OBJ vertex on line {line_number} is non-finite."
        )));
    }
    Ok(value)
}

fn bounds(positions: &[[f32; 3]]) -> CoreResult<Bounds> {
    if positions.is_empty() {
        return Err(output_error("Mesh positions are empty."));
    }
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for position in positions {
        if position.iter().any(|component| !component.is_finite()) {
            return Err(output_error("Mesh positions must be finite."));
        }
        for (axis, component) in position.iter().enumerate() {
            minimum[axis] = minimum[axis].min(*component as f64);
            maximum[axis] = maximum[axis].max(*component as f64);
        }
    }
    let mut span = [0.0; 3];
    let mut center = [0.0; 3];
    for axis in 0..3 {
        span[axis] = maximum[axis] - minimum[axis];
        center[axis] = minimum[axis] + span[axis] * 0.5;
    }
    let diagonal = dot(span, span).sqrt();
    if !diagonal.is_finite() || diagonal <= 1.0e-12 {
        return Err(output_error("Mesh bounds are degenerate."));
    }
    Ok(Bounds { center, diagonal })
}

fn point(value: [f32; 3]) -> [f64; 3] {
    [value[0] as f64, value[1] as f64, value[2] as f64]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f64; 3], scalar: f64) -> [f64; 3] {
    [a[0] * scalar, a[1] * scalar, a[2] * scalar]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn output_error(message: impl Into<String>) -> CoreError {
    CoreError::OperationUnavailable(message.into())
}

const MAX_INPUT_OBJ_BYTES: usize = 64 * 1024 * 1024;
const MAX_QUAD_TARGET: u32 = 50_000;
const QUADRIFLOW_TIMEOUT: Duration = Duration::from_secs(180);
const QUADRIFLOW_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuadRetopologyOptions {
    /// Approximate requested number of output quads; the upstream solver may miss it.
    pub target_quads: u32,
}

impl Default for QuadRetopologyOptions {
    fn default() -> Self {
        Self {
            target_quads: 10_000,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QuadRetopologyReport {
    pub algorithm: String,
    pub input_vertices: u64,
    pub input_triangles: u64,
    pub requested_quads: u32,
    pub output_vertices: u64,
    pub output_quads: u64,
    pub preview_triangles: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appearance_projection: Option<ProjectedMeshBakeReport>,
    pub warnings: Vec<String>,
}

fn build_quad_preview(
    source_glb: &[u8],
    mesh: &QuadMesh,
    project_appearance: bool,
) -> CoreResult<(Vec<u8>, Option<ProjectedMeshBakeReport>)> {
    if !project_appearance {
        return Ok((quad_mesh_preview_glb(mesh)?, None));
    }

    let target = quad_mesh_preview_geometry(mesh)?;
    let projected = bake_projected_mesh(
        source_glb,
        &target.positions,
        &target.normals,
        &target.indices,
        AtlasBakeOptions::default(),
        0.02,
    )
    .map_err(|error| {
        CoreError::OperationUnavailable(format!(
            "Retopology appearance projection failed closed: {error}"
        ))
    })?;
    let report = projected.report.clone();
    let preview = encode_projected_mesh_glb(&projected).map_err(|error| {
        CoreError::OperationUnavailable(format!(
            "Could not encode projected retopology preview: {error}"
        ))
    })?;
    if preview.len() > MAX_PREVIEW_BYTES {
        return Err(CoreError::OperationUnavailable(
            "Projected retopology preview exceeds 128 MiB.".into(),
        ));
    }
    Ok((preview, Some(report)))
}

/// Checks for a packaged, same-directory QuadriFlow sidecar without executing it.
pub fn quadriflow_sidecar_available() -> bool {
    quadriflow_sidecar_path().is_some()
}

/// Run the packaged sidecar on a generated geometry-only OBJ. The original OBJ
/// bytes are returned separately so callers can persist the actual editable quads;
/// the GLB is only a triangle preview.
fn validate_quad_target(target_quads: u32) -> CoreResult<()> {
    if !(4..=MAX_QUAD_TARGET).contains(&target_quads) {
        return Err(CoreError::InvalidInput(format!(
            "target_quads must be between 4 and {MAX_QUAD_TARGET}."
        )));
    }
    Ok(())
}

pub fn quadriflow_retopology_glb(
    bytes: &[u8],
    options: QuadRetopologyOptions,
) -> CoreResult<(Vec<u8>, Vec<u8>, QuadRetopologyReport)> {
    validate_quad_target(options.target_quads)?;

    let (positions, indices, project_appearance) = match extract_retopology_geometry(bytes) {
        Ok((positions, indices)) => (positions, indices, false),
        Err(_) => {
            let source = meshharbor_atlas_baker::extract_projection_source_mesh(bytes)
                    .map_err(|error| {
                        CoreError::OperationUnavailable(format!(
                            "Retopology accepts either supported untextured geometry or one static embedded PNG PBR mesh for projection: {error}"
                        ))
                    })?;
            (source.positions, source.indices, true)
        }
    };
    let validation = RemeshRequest {
        protocol_version: crate::REMESH_PROTOCOL_VERSION,
        positions: positions.clone(),
        indices: indices.clone(),
        // This conservative validation budget is independent of the QuadriFlow face target.
        // It rejects pathological input surface density before starting a native process.
        options: RetopologyOptions {
            target_edge_ratio: 0.25,
            iterations: 1,
        },
    };
    validate_remesh_request(&validation)?;

    let input_obj = triangle_geometry_obj(&positions, &indices)?;
    let worker = quadriflow_sidecar_path().ok_or_else(|| {
        CoreError::OperationUnavailable(
            "QuadriFlow is not packaged. Install the pinned geometry-only sidecar beside the MeshHarbor executable.".into(),
        )
    })?;
    let temp = QuadriflowTempDir::create()?;
    let input_path = temp.0.join("input.obj");
    let output_path = temp.0.join("output.obj");
    fs::write(&input_path, input_obj)?;

    let mut child = Command::new(&worker)
        .arg("-i")
        .arg(&input_path)
        .arg("-o")
        .arg(&output_path)
        .arg("-f")
        .arg(options.target_quads.to_string())
        .arg("-seed")
        .arg("0")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            CoreError::OperationUnavailable(format!(
                "Could not start the packaged QuadriFlow sidecar: {error}"
            ))
        })?;

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
        if started.elapsed() >= QUADRIFLOW_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CoreError::OperationUnavailable(
                "QuadriFlow exceeded its 180-second process limit.".into(),
            ));
        }
        thread::sleep(QUADRIFLOW_POLL_INTERVAL);
    };
    if !status.success() {
        return Err(CoreError::OperationUnavailable(format!(
            "Packaged QuadriFlow exited with {status}."
        )));
    }

    let output_size = fs::metadata(&output_path)?.len();
    if output_size == 0 || output_size > MAX_OBJ_BYTES as u64 {
        return Err(CoreError::OperationUnavailable(
            "QuadriFlow output is empty or exceeds 64 MiB.".into(),
        ));
    }
    let quad_obj = fs::read(&output_path)?;
    let (mesh, parsed_report) = parse_quad_obj_output(&quad_obj, &positions)?;
    let (preview_glb, appearance_projection) =
        build_quad_preview(bytes, &mesh, project_appearance)?;

    let mut warnings = parsed_report.warnings;
    warnings.push(
        "The OBJ preserves editable quad topology only; the GLB is a triangulated viewer preview."
            .into(),
    );
    if project_appearance {
        warnings.extend([
            "The preview projects only embedded base-color and metallic-roughness PNGs; UVs, colors, and other vertex streams are not transferred to the OBJ.".into(),
            "Projection is nearest-surface with a 2% source-bounds distance cap, similarly-facing normal gate, 99.5% coverage threshold, and thin-shell ambiguity rejection; passing does not certify visual texture quality.".into(),
        ]);
    } else {
        warnings.push("This geometry-only input has no appearance maps to project.".into());
    }
    warnings.extend([
        "This candidate does not certify self-intersection freedom or a measured surface-error tolerance; inspect the result before production use.".into(),
        "The only native end-to-end QuadriFlow smoke test used a synthetic cube; broad model quality and platform packaging remain unverified.".into(),
    ]);
    if parsed_report.output_quads != u64::from(options.target_quads) {
        warnings.push(format!(
            "The solver returned {} quads for a target of {}; the target is approximate.",
            parsed_report.output_quads, options.target_quads
        ));
    }
    let report = QuadRetopologyReport {
        algorithm: "quadriflow_quad_retopology".into(),
        input_vertices: positions.len() as u64,
        input_triangles: (indices.len() / 3) as u64,
        requested_quads: options.target_quads,
        output_vertices: parsed_report.output_vertices,
        output_quads: parsed_report.output_quads,
        preview_triangles: parsed_report.preview_triangles,
        appearance_projection,
        warnings,
    };
    Ok((preview_glb, quad_obj, report))
}

fn quadriflow_sidecar_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let name = format!("quadriflow{}", std::env::consts::EXE_SUFFIX);
    let sidecar = executable.with_file_name(name);
    let metadata = fs::metadata(&sidecar).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(sidecar)
}

struct QuadriflowTempDir(PathBuf);

impl QuadriflowTempDir {
    fn create() -> CoreResult<Self> {
        for _ in 0..4 {
            let path =
                std::env::temp_dir().join(format!("meshharbor-quadriflow-{}", Uuid::new_v4()));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(CoreError::OperationUnavailable(
            "Could not allocate a private QuadriFlow working directory.".into(),
        ))
    }
}

impl Drop for QuadriflowTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn triangle_geometry_obj(positions: &[[f32; 3]], indices: &[u32]) -> CoreResult<String> {
    if positions.is_empty()
        || positions.len() > MAX_VERTICES
        || indices.is_empty()
        || !indices.len().is_multiple_of(3)
        || indices.len() / 3 > 200_000
    {
        return Err(CoreError::OperationUnavailable(
            "QuadriFlow input exceeds the static geometry limits.".into(),
        ));
    }
    let mut text = String::new();
    for position in positions {
        if position.iter().any(|component| !component.is_finite()) {
            return Err(CoreError::InvalidInput(
                "QuadriFlow input positions must be finite.".into(),
            ));
        }
        writeln!(
            &mut text,
            "v {} {} {}",
            position[0], position[1], position[2]
        )
        .expect("writing to a String cannot fail");
        if text.len() > MAX_INPUT_OBJ_BYTES {
            return Err(CoreError::OperationUnavailable(
                "Generated QuadriFlow OBJ input exceeds 64 MiB.".into(),
            ));
        }
    }
    for triangle in indices.chunks_exact(3) {
        if triangle
            .iter()
            .any(|index| *index as usize >= positions.len())
        {
            return Err(CoreError::InvalidInput(
                "QuadriFlow input index references a missing position.".into(),
            ));
        }
        writeln!(
            &mut text,
            "f {} {} {}",
            triangle[0] + 1,
            triangle[1] + 1,
            triangle[2] + 1
        )
        .expect("writing to a String cannot fail");
        if text.len() > MAX_INPUT_OBJ_BYTES {
            return Err(CoreError::OperationUnavailable(
                "Generated QuadriFlow OBJ input exceeds 64 MiB.".into(),
            ));
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE_OBJ: &str = "v -1 -1 -1\n\
v 1 -1 -1\n\
v 1 1 -1\n\
v -1 1 -1\n\
v -1 -1 1\n\
v 1 -1 1\n\
v 1 1 1\n\
v -1 1 1\n\
f 1 4 3 2\n\
f 5 6 7 8\n\
f 1 2 6 5\n\
f 2 3 7 6\n\
f 3 4 8 7\n\
f 4 1 5 8\n";

    fn cube_positions() -> [[f32; 3]; 8] {
        [
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ]
    }

    const BASE_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6,
        0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 20, 73, 68, 65, 84, 120, 218, 99, 252, 47, 199, 245,
        159, 129, 129, 129, 129, 137, 1, 10, 0, 33, 141, 2, 42, 48, 155, 50, 207, 0, 0, 0, 0, 73,
        69, 78, 68, 174, 66, 96, 130,
    ];
    const MR_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6,
        0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 20, 73, 68, 65, 84, 120, 218, 99, 100, 104, 248, 255,
        159, 129, 129, 129, 129, 137, 1, 10, 0, 37, 25, 2, 130, 79, 188, 53, 61, 0, 0, 0, 0, 73,
        69, 78, 68, 174, 66, 96, 130,
    ];

    fn append_view(
        binary: &mut Vec<u8>,
        views: &mut Vec<serde_json::Value>,
        bytes: &[u8],
        target: Option<u64>,
    ) -> usize {
        while !binary.len().is_multiple_of(4) {
            binary.push(0);
        }
        let offset = binary.len();
        binary.extend_from_slice(bytes);
        let mut view = json!({"buffer":0,"byteOffset":offset,"byteLength":bytes.len()});
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        views.push(view);
        views.len() - 1
    }

    fn floats(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    fn textured_square_glb() -> Vec<u8> {
        let mut binary = Vec::new();
        let mut views = Vec::new();
        let positions = [
            0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0,
        ];
        let normals = [
            0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0,
        ];
        let uv = [0.0f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        let position_view = append_view(&mut binary, &mut views, &floats(&positions), Some(34962));
        let normal_view = append_view(&mut binary, &mut views, &floats(&normals), Some(34962));
        let uv_view = append_view(&mut binary, &mut views, &floats(&uv), Some(34962));
        let index_bytes: Vec<u8> = [0u16, 1, 2, 0, 2, 3]
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect();
        let index_view = append_view(&mut binary, &mut views, &index_bytes, Some(34963));
        let base_view = append_view(&mut binary, &mut views, BASE_PNG, None);
        let mr_view = append_view(&mut binary, &mut views, MR_PNG, None);
        let document = json!({
            "asset":{"version":"2.0"},
            "buffers":[{"byteLength":binary.len()}],
            "bufferViews":views,
            "accessors":[
                {"bufferView":position_view,"componentType":5126,"count":4,"type":"VEC3"},
                {"bufferView":normal_view,"componentType":5126,"count":4,"type":"VEC3"},
                {"bufferView":uv_view,"componentType":5126,"count":4,"type":"VEC2"},
                {"bufferView":index_view,"componentType":5123,"count":6,"type":"SCALAR"}
            ],
            "images":[{"bufferView":base_view,"mimeType":"image/png"},{"bufferView":mr_view,"mimeType":"image/png"}],
            "textures":[{"source":0},{"source":1}],
            "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0},"metallicRoughnessTexture":{"index":1}}}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3,"material":0,"mode":4}]}],
            "nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}],"scene":0
        });
        let mut json_bytes = serde_json::to_vec(&document).unwrap();
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        while !binary.len().is_multiple_of(4) {
            binary.push(0);
        }
        let total_length = 12 + 8 + json_bytes.len() + 8 + binary.len();
        let mut glb = Vec::with_capacity(total_length);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total_length as u32).to_le_bytes());
        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x004E4942u32.to_le_bytes());
        glb.extend_from_slice(&binary);
        glb
    }

    fn preview_json(bytes: &[u8]) -> serde_json::Value {
        assert_eq!(&bytes[..4], b"glTF");
        let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        serde_json::from_slice(&bytes[20..20 + length]).unwrap()
    }

    #[test]
    fn bounds_quad_target_and_rejects_unknown_parameters() {
        assert!(validate_quad_target(4).is_ok());
        assert!(validate_quad_target(MAX_QUAD_TARGET).is_ok());
        assert!(validate_quad_target(3).is_err());
        assert!(validate_quad_target(MAX_QUAD_TARGET + 1).is_err());
        assert!(
            serde_json::from_value::<QuadRetopologyOptions>(
                json!({"target_quads": 12, "arbitrary_flag": true})
            )
            .is_err()
        );
    }

    #[test]
    fn generates_only_geometry_records_for_the_native_input() {
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let indices = [0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];
        let obj = triangle_geometry_obj(&positions, &indices).unwrap();
        assert_eq!(obj.lines().filter(|line| line.starts_with("v ")).count(), 4);
        assert_eq!(obj.lines().filter(|line| line.starts_with("f ")).count(), 4);
        assert!(obj.contains("f 1 3 2\n"));
        assert!(!obj.contains("vt "));
        assert!(!obj.contains("vn "));
    }

    #[test]
    fn parses_closed_quad_obj_and_writes_triangle_preview_glb() {
        let (mesh, report) = parse_quad_obj_output(CUBE_OBJ.as_bytes(), &cube_positions()).unwrap();
        assert_eq!(mesh.vertex_count(), 8);
        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(report.preview_triangles, 12);

        let preview = quad_mesh_preview_glb(&mesh).unwrap();
        let inspected = crate::inspect_asset("cube-preview.glb", &preview).unwrap();
        assert_eq!(inspected.kind, crate::AssetKind::Mesh);
        assert_eq!(inspected.metadata["triangles"], 12);
    }

    #[test]
    fn textured_quad_preview_projects_supported_maps_and_reports_coverage() {
        let source = textured_square_glb();
        let mesh = QuadMesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            quads: vec![[0, 1, 2, 3]],
        };

        let (preview, report) = build_quad_preview(&source, &mesh, true).unwrap();
        let report = report.unwrap();
        assert!(report.projection_coverage >= 0.995);
        assert_eq!(report.ambiguous_texels, 0);
        assert_eq!(
            report.rebaked_maps,
            ["baseColorTexture", "metallicRoughnessTexture"]
        );
        let document = preview_json(&preview);
        assert_eq!(document["images"].as_array().unwrap().len(), 2);
        assert_eq!(document["textures"].as_array().unwrap().len(), 2);
        assert_eq!(document["meshes"][0]["primitives"][0]["mode"], 4);
        assert!(preview.len() <= MAX_PREVIEW_BYTES);
    }

    #[test]
    fn projection_failure_does_not_fall_back_to_an_untextured_preview() {
        let source = textured_square_glb();
        let mesh = QuadMesh {
            positions: vec![
                [0.0, 0.0, 0.2],
                [1.0, 0.0, 0.2],
                [1.0, 1.0, 0.2],
                [0.0, 1.0, 0.2],
            ],
            quads: vec![[0, 1, 2, 3]],
        };

        let error = build_quad_preview(&source, &mesh, true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("appearance projection failed closed")
        );
    }

    #[test]
    fn rejects_non_quad_or_open_output() {
        let non_quad = CUBE_OBJ.replacen("f 1 4 3 2", "f 1 4 3", 1);
        assert!(
            parse_quad_obj_output(non_quad.as_bytes(), &cube_positions())
                .unwrap_err()
                .to_string()
                .contains("four")
        );

        let open = CUBE_OBJ.replacen("f 4 1 5 8\n", "", 1);
        assert!(
            parse_quad_obj_output(open.as_bytes(), &cube_positions())
                .unwrap_err()
                .to_string()
                .contains("open or non-watertight")
        );
    }

    #[test]
    fn rejects_unsupported_obj_records_and_bad_indices() {
        let with_uv = CUBE_OBJ.replace("v -1 -1 -1", "vt 0 0\nv -1 -1 -1");
        assert!(
            parse_quad_obj_output(with_uv.as_bytes(), &cube_positions())
                .unwrap_err()
                .to_string()
                .contains("unsupported record")
        );

        let bad_index = CUBE_OBJ.replacen("f 1 4 3 2", "f 1 4 3 0", 1);
        assert!(
            parse_quad_obj_output(bad_index.as_bytes(), &cube_positions())
                .unwrap_err()
                .to_string()
                .contains("zero index")
        );
    }

    #[test]
    fn rejects_output_far_outside_source_bounds() {
        let far = CUBE_OBJ.replace("v 1 1 1", "v 100 100 100");
        assert!(
            parse_quad_obj_output(far.as_bytes(), &cube_positions())
                .unwrap_err()
                .to_string()
                .contains("envelope")
        );
    }
}
