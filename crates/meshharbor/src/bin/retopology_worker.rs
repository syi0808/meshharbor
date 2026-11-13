//! Optional child-process entry point for experimental triangle remeshing.
//!
//! The host and worker share a small versioned JSON protocol. This executable is
//! deliberately separate from the REST/MCP process because remesh 0.0.5 is
//! experimental, may panic, and writes progress to stdout.
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use clap::Parser;
use remesh::prelude::*;
use serde::{Deserialize, Serialize};

const PROTOCOL_VERSION: u32 = 1;
const MAX_REQUEST_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024 * 1024;
const MAX_VERTICES: usize = 200_000;
const MAX_TRIANGLES: usize = 400_000;
const MAX_OUTPUT_VERTICES: usize = 1_000_000;
const MAX_OUTPUT_TRIANGLES: usize = 2_000_000;

#[derive(Debug, Parser)]
#[command(about = "MeshHarbor experimental triangle-remeshing worker")]
struct Args {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Options {
    target_edge_ratio: f32,
    iterations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Request {
    protocol_version: u32,
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
    options: Options,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Response {
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("meshharbor-retopo-worker: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse();
    let request_size = fs::metadata(&args.input)
        .map_err(|error| format!("cannot stat request file: {error}"))?
        .len();
    if request_size > MAX_REQUEST_BYTES {
        return Err("request file exceeds the 128 MiB limit".into());
    }
    let request_bytes =
        fs::read(&args.input).map_err(|error| format!("cannot read request file: {error}"))?;
    let request: Request = serde_json::from_slice(&request_bytes)
        .map_err(|error| format!("invalid request JSON: {error}"))?;

    let response = remesh(request)?;
    let output_bytes = serde_json::to_vec(&response)
        .map_err(|error| format!("cannot encode response: {error}"))?;
    if output_bytes.len() > MAX_RESPONSE_BYTES {
        return Err("response exceeds the 256 MiB limit".into());
    }

    // Never overwrite a caller-selected existing file. The host supplies a fresh
    // response path inside its private temporary directory.
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args.output)
        .map_err(|error| format!("cannot create response file: {error}"))?;
    output
        .write_all(&output_bytes)
        .map_err(|error| format!("cannot write response file: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("cannot flush response file: {error}"))?;
    Ok(())
}

fn remesh(request: Request) -> Result<Response, String> {
    validate_request(&request)?;
    let (normalized, center, scale_factor, diagonal) = normalize(&request)?;
    // remesh uses u32 indices for this API version. Keep the GLB index width
    // throughout instead of round-tripping through usize.
    let remesher = IsotropicRemesh::<f32, [f32; 3]>::new(&normalized, &request.indices)
        .map_err(|error| format!("remesh input rejected: {error}"))?
        .with_target_edge_length(1.0)
        .map_err(|error| format!("invalid remesh target edge length: {error}"))?
        .with_split_edges(SplitStrategy::DihedralAngle)
        .map_err(|error| format!("cannot configure edge splitting: {error}"))?
        .with_collapse_edges(CollapseStrategy::DihedralAngle)
        .map_err(|error| format!("cannot configure edge collapsing: {error}"))?
        .with_flip_edges(FlipStrategy::Valence)
        .map_err(|error| format!("cannot configure edge flipping: {error}"))?
        .with_default_smooth_weight()
        .map_err(|error| format!("cannot configure smoothing: {error}"))?;

    let (output_positions, output_indices): (Vec<[f32; 3]>, Vec<u32>) = remesher
        .run(request.options.iterations as usize)
        .map_err(|error| format!("isotropic remeshing failed: {error}"))?;

    let positions = output_positions
        .into_iter()
        .map(|point| {
            [
                point[0] / scale_factor + center[0],
                point[1] / scale_factor + center[1],
                point[2] / scale_factor + center[2],
            ]
        })
        .collect::<Vec<_>>();
    let response = Response {
        positions,
        indices: output_indices,
    };
    validate_response(&response, center, diagonal)?;
    Ok(response)
}

fn validate_request(request: &Request) -> Result<(), String> {
    if request.protocol_version != PROTOCOL_VERSION {
        return Err(format!(
            "unsupported protocol version {}",
            request.protocol_version
        ));
    }
    if !request.options.target_edge_ratio.is_finite()
        || !(0.005..=0.25).contains(&request.options.target_edge_ratio)
    {
        return Err("target_edge_ratio must be finite and between 0.005 and 0.25".into());
    }
    if !(1..=8).contains(&request.options.iterations) {
        return Err("iterations must be between 1 and 8".into());
    }
    if request.positions.is_empty() || request.positions.len() > MAX_VERTICES {
        return Err(format!("request must contain 1 to {MAX_VERTICES} vertices"));
    }
    if request.indices.is_empty()
        || request.indices.len() % 3 != 0
        || request.indices.len() / 3 > MAX_TRIANGLES
    {
        return Err(format!(
            "request must contain 1 to {MAX_TRIANGLES} indexed triangles"
        ));
    }
    if request
        .positions
        .iter()
        .flatten()
        .any(|component| !component.is_finite())
    {
        return Err("all position components must be finite".into());
    }

    for index in &request.indices {
        if *index as usize >= request.positions.len() {
            return Err("an input index references a missing vertex".into());
        }
    }

    let mut edge_faces = HashMap::<(u32, u32), (u8, i8)>::new();
    let mut unique_faces = HashSet::<[u32; 3]>::new();
    let (_, diagonal) = bounds(&request.positions)?;
    for triangle in request.indices.chunks_exact(3) {
        let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
        if a == b || b == c || c == a {
            return Err("input contains a degenerate triangle".into());
        }
        let mut face_key = [a, b, c];
        face_key.sort_unstable();
        if !unique_faces.insert(face_key) {
            return Err("input contains duplicate triangles".into());
        }
        let inverse_diagonal = 1.0 / diagonal;
        let pa = request.positions[a as usize];
        let pb = request.positions[b as usize];
        let pc = request.positions[c as usize];
        let edge_a = scaled(sub(pb, pa), inverse_diagonal);
        let edge_b = scaled(sub(pc, pa), inverse_diagonal);
        if length_squared(cross(edge_a, edge_b)) <= 1.0e-16 {
            return Err("input contains a zero-area or near-degenerate triangle".into());
        }
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let edge = if u < v { (u, v) } else { (v, u) };
            let (count, winding) = edge_faces.entry(edge).or_default();
            *count = count.saturating_add(1);
            *winding += if u < v { 1 } else { -1 };
            if *count > 2 {
                return Err("input is not edge-manifold".into());
            }
        }
    }
    if edge_faces.values().any(|(count, _)| *count != 2) {
        return Err("input must be a closed, watertight triangle mesh".into());
    }
    if edge_faces.values().any(|(_, winding)| *winding != 0) {
        return Err("input faces must have consistent orientation".into());
    }
    Ok(())
}

fn normalize(request: &Request) -> Result<(Vec<[f32; 3]>, [f32; 3], f32, f32), String> {
    let (center, diagonal) = bounds(&request.positions)?;
    // remesh has an area/projection threshold expressed in default-edge units.
    // Scale uniformly so the requested edge length is exactly 1 in worker space.
    let scale_factor = 1.0 / (diagonal * request.options.target_edge_ratio);
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return Err("input normalization scale is invalid".into());
    }
    let normalized = request
        .positions
        .iter()
        .map(|point| {
            [
                (point[0] - center[0]) * scale_factor,
                (point[1] - center[1]) * scale_factor,
                (point[2] - center[2]) * scale_factor,
            ]
        })
        .collect();
    Ok((normalized, center, scale_factor, diagonal))
}

fn validate_response(response: &Response, center: [f32; 3], diagonal: f32) -> Result<(), String> {
    if response.positions.is_empty()
        || response.positions.len() > MAX_OUTPUT_VERTICES
        || response.indices.is_empty()
        || response.indices.len() % 3 != 0
        || response.indices.len() / 3 > MAX_OUTPUT_TRIANGLES
    {
        return Err("remesher returned an empty or oversized mesh".into());
    }
    if response
        .positions
        .iter()
        .flatten()
        .any(|component| !component.is_finite())
    {
        return Err("remesher returned a non-finite position".into());
    }
    if response
        .positions
        .iter()
        .any(|point| squared_distance(*point, center) / (diagonal * diagonal) > 4.0)
    {
        return Err("remesher output escaped the source mesh bounds".into());
    }
    if response
        .indices
        .iter()
        .any(|index| *index as usize >= response.positions.len())
    {
        return Err("remesher returned an out-of-range index".into());
    }

    let mut edge_faces = HashMap::<(u32, u32), u8>::new();
    let mut unique_faces = HashSet::<[u32; 3]>::new();
    let inverse_diagonal = 1.0 / diagonal;
    for triangle in response.indices.chunks_exact(3) {
        let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
        if a == b || b == c || c == a {
            return Err("remesher returned a degenerate triangle".into());
        }
        let mut face_key = [a, b, c];
        face_key.sort_unstable();
        if !unique_faces.insert(face_key) {
            return Err("remesher returned duplicate triangles".into());
        }
        let pa = response.positions[a as usize];
        let pb = response.positions[b as usize];
        let pc = response.positions[c as usize];
        let edge_a = scaled(sub(pb, pa), inverse_diagonal);
        let edge_b = scaled(sub(pc, pa), inverse_diagonal);
        if length_squared(cross(edge_a, edge_b)) <= 1.0e-16 {
            return Err("remesher returned a zero-area or near-degenerate triangle".into());
        }
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let edge = if u < v { (u, v) } else { (v, u) };
            let (count, winding) = edge_faces.entry(edge).or_default();
            *count = count.saturating_add(1);
            *winding += if u < v { 1 } else { -1 };
            if *count > 2 {
                return Err("remesher returned a non-manifold edge".into());
            }
        }
    }
    if edge_faces.values().any(|(count, _)| *count != 2) {
        return Err("remesher returned an open or non-watertight mesh".into());
    }
    if edge_faces.values().any(|(_, winding)| *winding != 0) {
        return Err("remesher returned inconsistently oriented faces".into());
    }
    Ok(())
}

fn bounds(positions: &[[f32; 3]]) -> Result<([f32; 3], f32), String> {
    if positions.is_empty() {
        return Err("input bounds are empty".into());
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    let span = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let diagonal = (span[0] * span[0] + span[1] * span[1] + span[2] * span[2]).sqrt();
    if !diagonal.is_finite() || diagonal <= 1.0e-12 {
        return Err("input bounds are degenerate".into());
    }
    let center = [
        min[0] + span[0] * 0.5,
        min[1] + span[1] * 0.5,
        min[2] + span[2] * 0.5,
    ];
    Ok((center, diagonal))
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scaled(a: [f32; 3], value: f32) -> [f32; 3] {
    [a[0] * value, a[1] * value, a[2] * value]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length_squared(a: [f32; 3]) -> f32 {
    a[0] * a[0] + a[1] * a[1] + a[2] * a[2]
}

fn squared_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remeshes_an_untextured_triangle_patch() {
        let request = Request {
            protocol_version: PROTOCOL_VERSION,
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            options: Options {
                target_edge_ratio: 0.2,
                iterations: 1,
            },
        };
        let output = remesh(request).expect("isotropic remeshing should produce a triangle mesh");
        assert!(!output.positions.is_empty());
        assert!(!output.indices.is_empty());
        assert_eq!(output.indices.len() % 3, 0);
        assert!(
            output
                .indices
                .iter()
                .all(|index| (*index as usize) < output.positions.len())
        );
    }
}
