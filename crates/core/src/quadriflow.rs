//! Strict geometry-only QuadriFlow OBJ result validation and GLB preview export.
//!
//! GLB can only store triangle primitives. Keep the QuadriFlow OBJ as the editable
//! quad-topology artifact and use the GLB produced here only as a viewer preview.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::json;

use crate::{CoreError, CoreResult};

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

    let mut normals = vec![[0.0f64; 3]; mesh.positions.len()];
    for triangle in triangle_indices.chunks_exact(3) {
        let a = point(mesh.positions[triangle[0] as usize]);
        let b = point(mesh.positions[triangle[1] as usize]);
        let c = point(mesh.positions[triangle[2] as usize]);
        let normal = cross(sub(b, a), sub(c, a));
        for index in triangle {
            let accumulated = &mut normals[*index as usize];
            for axis in 0..3 {
                accumulated[axis] += normal[axis];
            }
        }
    }

    let mut packed_normals = Vec::with_capacity(mesh.positions.len());
    for normal in normals {
        let length = dot(normal, normal).sqrt();
        if !length.is_finite() || length <= 1.0e-18 {
            return Err(output_error(
                "Quad preview contains a vertex with an undefined normal.",
            ));
        }
        packed_normals.push([
            (normal[0] / length) as f32,
            (normal[1] / length) as f32,
            (normal[2] / length) as f32,
        ]);
    }

    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for position in &mesh.positions {
        for (axis, component) in position.iter().enumerate() {
            minimum[axis] = minimum[axis].min(*component);
            maximum[axis] = maximum[axis].max(*component);
        }
    }

    let position_byte_length = mesh.positions.len() * 12;
    let normal_byte_offset = position_byte_length;
    let normal_byte_length = packed_normals.len() * 12;
    let index_byte_offset = normal_byte_offset + normal_byte_length;
    let index_byte_length = triangle_indices.len() * 4;
    let mut binary = Vec::with_capacity(index_byte_offset + index_byte_length);

    for position in &mesh.positions {
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
                "count": mesh.positions.len(),
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
                "max": [mesh.positions.len() - 1]
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
