//! Experimental process-isolated isotropic triangle remeshing worker adapter.
//!
//! This is not field-aligned quad retopology. The worker accepts raw indexed
//! triangle geometry; the GLB caller must reject textured/attribute-rich assets
//! until a seam-aware high-to-low bake is available.

use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{CoreError, CoreResult};

pub const REMESH_PROTOCOL_VERSION: u32 = 1;
const MAX_VERTICES: usize = 200_000;
const MAX_TRIANGLES: usize = 400_000;
const MAX_OUTPUT_VERTICES: usize = 1_000_000;
const MAX_OUTPUT_TRIANGLES: usize = 2_000_000;
const MAX_REQUEST_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 256 * 1024 * 1024;
const WORKER_TIMEOUT: Duration = Duration::from_secs(180);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct RetopologyOptions {
    /// Target edge length as a fraction of the input bounding-box diagonal.
    pub target_edge_ratio: f32,
    /// Remesh iterations. Kept small because the upstream algorithm is experimental.
    pub iterations: u32,
}

impl Default for RetopologyOptions {
    fn default() -> Self {
        Self {
            target_edge_ratio: 0.05,
            iterations: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemeshRequest {
    pub protocol_version: u32,
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub options: RetopologyOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemeshResponse {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetopologyReport {
    pub algorithm: String,
    pub input_vertices: u64,
    pub input_triangles: u64,
    pub output_vertices: u64,
    pub output_triangles: u64,
    pub target_edge_length: f32,
    pub iterations: u32,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy)]
struct Bounds {
    center: [f32; 3],
    diagonal: f32,
}

struct WorkerTempDir(PathBuf);

impl WorkerTempDir {
    fn create() -> CoreResult<Self> {
        for _ in 0..4 {
            let path = std::env::temp_dir().join(format!("meshharbor-retopo-{}", Uuid::new_v4()));
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
            "Could not allocate an isolated retopology-worker directory.".into(),
        ))
    }
}

impl Drop for WorkerTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Run the optional worker as a separate process. Its stdout/stderr are redirected
/// to temporary files so the upstream crate's unconditional println! cannot corrupt
/// the MCP stdio protocol or application logs.
pub fn remesh_triangles_in_worker(
    positions: &[[f32; 3]],
    indices: &[u32],
    options: RetopologyOptions,
) -> CoreResult<(RemeshResponse, RetopologyReport)> {
    let request = RemeshRequest {
        protocol_version: REMESH_PROTOCOL_VERSION,
        positions: positions.to_vec(),
        indices: indices.to_vec(),
        options,
    };
    validate_remesh_request(&request)?;
    let bounds = geometry_bounds(&request.positions)?;

    let request_bytes = serde_json::to_vec(&request)?;
    if request_bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err(CoreError::OperationUnavailable(
            "Remesh worker request exceeds the size limit.".into(),
        ));
    }

    let worker = worker_path()?;
    let temp = WorkerTempDir::create()?;
    let input_path = temp.0.join("request.json");
    let output_path = temp.0.join("response.json");
    let stdout_path = temp.0.join("worker.stdout");
    let stderr_path = temp.0.join("worker.stderr");
    fs::write(&input_path, request_bytes)?;

    let stdout = fs::File::create(&stdout_path)?;
    let stderr = fs::File::create(&stderr_path)?;
    let mut child = Command::new(&worker)
        .arg("--input")
        .arg(&input_path)
        .arg("--output")
        .arg(&output_path)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| {
            CoreError::OperationUnavailable(format!(
                "Could not start the optional retopology worker: {error}"
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
        if started.elapsed() >= WORKER_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CoreError::OperationUnavailable(
                "Experimental remeshing exceeded its 180-second worker limit.".into(),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    };

    if !status.success() {
        let stdout_detail = read_log_excerpt(&stdout_path);
        let stderr_detail = read_log_excerpt(&stderr_path);
        return Err(CoreError::OperationUnavailable(format!(
            "Experimental remesh worker exited with {status}. stdout: {stdout_detail}; stderr: {stderr_detail}"
        )));
    }

    let output_size = fs::metadata(&output_path)?.len();
    if output_size > MAX_RESPONSE_BYTES {
        return Err(CoreError::OperationUnavailable(
            "Remesh worker output exceeds the size limit.".into(),
        ));
    }
    let output_bytes = fs::read(&output_path)?;
    let response: RemeshResponse = serde_json::from_slice(&output_bytes)?;
    validate_remesh_response(&response, &request)?;

    let report = RetopologyReport {
        algorithm: "isotropic_triangle_remeshing".into(),
        input_vertices: request.positions.len() as u64,
        input_triangles: (request.indices.len() / 3) as u64,
        output_vertices: response.positions.len() as u64,
        output_triangles: (response.indices.len() / 3) as u64,
        target_edge_length: bounds.diagonal * options.target_edge_ratio,
        iterations: options.iterations,
        warnings: vec![
            "Experimental isotropic triangle remeshing; output remains triangles and is not field-aligned quad retopology.".into(),
            "The worker processes geometry only. The GLB adapter must reject UVs, textures, skinning, morphs, and unsupported vertex attributes until high-to-low appearance transfer is implemented.".into(),
        ],
    };
    Ok((response, report))
}

fn worker_path() -> CoreResult<PathBuf> {
    let executable = std::env::current_exe()?;
    let name = format!("meshharbor-retopo-worker{}", std::env::consts::EXE_SUFFIX);
    let worker = executable.with_file_name(name);
    if !worker.is_file() {
        return Err(CoreError::OperationUnavailable(
            "The experimental retopology worker is not installed beside MeshHarbor. Build it with cargo build -p meshharbor --features experimental-retopology-worker.".into(),
        ));
    }
    Ok(worker)
}

fn read_log_excerpt(path: &PathBuf) -> String {
    let Ok(file) = fs::File::open(path) else {
        return String::new();
    };
    let mut bytes = Vec::new();
    if file.take(4097).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    if bytes.len() > 4096 {
        bytes.truncate(4096);
        bytes.extend_from_slice(b" ...[truncated]");
    }
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

pub fn validate_remesh_request(request: &RemeshRequest) -> CoreResult<()> {
    if request.protocol_version != REMESH_PROTOCOL_VERSION {
        return Err(CoreError::InvalidInput(format!(
            "Unsupported remesh-worker protocol version {}.",
            request.protocol_version
        )));
    }
    if !request.options.target_edge_ratio.is_finite()
        || !(0.005..=0.25).contains(&request.options.target_edge_ratio)
    {
        return Err(CoreError::InvalidInput(
            "target_edge_ratio must be finite and between 0.005 and 0.25.".into(),
        ));
    }
    if !(1..=8).contains(&request.options.iterations) {
        return Err(CoreError::InvalidInput(
            "Remesh iterations must be between 1 and 8.".into(),
        ));
    }
    if request.positions.is_empty() || request.positions.len() > MAX_VERTICES {
        return Err(CoreError::OperationUnavailable(format!(
            "Remesh accepts 1 to {MAX_VERTICES} positions."
        )));
    }
    if request.indices.is_empty()
        || !request.indices.len().is_multiple_of(3)
        || request.indices.len() / 3 > MAX_TRIANGLES
    {
        return Err(CoreError::InvalidInput(format!(
            "Remesh requires 1 to {MAX_TRIANGLES} indexed triangles."
        )));
    }
    for position in &request.positions {
        if position.iter().any(|component| !component.is_finite()) {
            return Err(CoreError::InvalidInput(
                "Remesh positions must be finite.".into(),
            ));
        }
    }
    for index in &request.indices {
        if *index as usize >= request.positions.len() {
            return Err(CoreError::InvalidInput(
                "Remesh index references a missing position.".into(),
            ));
        }
    }

    let bounds = geometry_bounds(&request.positions)?;
    let inverse_diagonal = 1.0 / bounds.diagonal;
    let mut edge_faces = std::collections::HashMap::<(u32, u32), (u8, i8)>::new();
    let mut unique_faces = std::collections::HashSet::<[u32; 3]>::new();
    for triangle in request.indices.chunks_exact(3) {
        let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
        if a == b || b == c || c == a {
            return Err(CoreError::InvalidInput(
                "Remesh input contains a degenerate triangle.".into(),
            ));
        }
        let mut face_key = [a, b, c];
        face_key.sort_unstable();
        if !unique_faces.insert(face_key) {
            return Err(CoreError::InvalidInput(
                "Remesh input contains duplicate triangles.".into(),
            ));
        }
        let pa = request.positions[a as usize];
        let pb = request.positions[b as usize];
        let pc = request.positions[c as usize];
        let edge_a = scale(sub(pb, pa), inverse_diagonal);
        let edge_b = scale(sub(pc, pa), inverse_diagonal);
        let cross_product = cross(edge_a, edge_b);
        if dot(cross_product, cross_product) <= 1.0e-16 {
            return Err(CoreError::InvalidInput(
                "Remesh input contains a zero-area or near-degenerate triangle.".into(),
            ));
        }
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let key = if u < v { (u, v) } else { (v, u) };
            let (count, winding) = edge_faces.entry(key).or_default();
            *count = count.saturating_add(1);
            *winding += if u < v { 1 } else { -1 };
            if *count > 2 {
                return Err(CoreError::OperationUnavailable(
                    "The experimental remesher only accepts edge-manifold triangle meshes.".into(),
                ));
            }
        }
    }
    if edge_faces.values().any(|(count, _)| *count != 2) {
        return Err(CoreError::OperationUnavailable(
            "The experimental remesher only accepts closed, watertight triangle meshes.".into(),
        ));
    }
    if edge_faces.values().any(|(_, winding)| *winding != 0) {
        return Err(CoreError::OperationUnavailable(
            "The experimental remesher requires consistently oriented triangle faces.".into(),
        ));
    }
    Ok(())
}

pub fn normalize_remesh_positions(
    request: &RemeshRequest,
) -> CoreResult<(Vec<[f32; 3]>, [f32; 3], f32)> {
    validate_remesh_request(request)?;
    let bounds = geometry_bounds(&request.positions)?;
    // The upstream crate leaves area/projection thresholds in default-edge units.
    // Scale uniformly so the requested edge length is exactly one in worker space.
    let scale_factor = 1.0 / (bounds.diagonal * request.options.target_edge_ratio);
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return Err(CoreError::InvalidInput(
            "Remesh normalization scale is invalid.".into(),
        ));
    }
    let normalized = request
        .positions
        .iter()
        .map(|position| scale(sub(*position, bounds.center), scale_factor))
        .collect();
    Ok((normalized, bounds.center, scale_factor))
}

pub fn restore_remesh_positions(
    positions: &[[f32; 3]],
    center: [f32; 3],
    scale_factor: f32,
) -> CoreResult<Vec<[f32; 3]>> {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return Err(CoreError::InvalidInput(
            "Remesh restore scale is invalid.".into(),
        ));
    }
    let restored: Vec<_> = positions
        .iter()
        .map(|position| {
            [
                position[0] / scale_factor + center[0],
                position[1] / scale_factor + center[1],
                position[2] / scale_factor + center[2],
            ]
        })
        .collect();
    if restored
        .iter()
        .any(|position| position.iter().any(|component| !component.is_finite()))
    {
        return Err(CoreError::OperationUnavailable(
            "Remesh worker produced non-finite output positions.".into(),
        ));
    }
    Ok(restored)
}

pub fn validate_remesh_response(
    response: &RemeshResponse,
    request: &RemeshRequest,
) -> CoreResult<()> {
    if response.positions.is_empty()
        || response.positions.len() > MAX_OUTPUT_VERTICES
        || response.indices.is_empty()
        || response.indices.len() % 3 != 0
        || response.indices.len() / 3 > MAX_OUTPUT_TRIANGLES
    {
        return Err(CoreError::OperationUnavailable(
            "Remesh worker returned an empty or oversized mesh.".into(),
        ));
    }
    let bounds = geometry_bounds(&request.positions)?;
    for position in &response.positions {
        if position.iter().any(|component| !component.is_finite()) {
            return Err(CoreError::OperationUnavailable(
                "Remesh worker returned non-finite positions.".into(),
            ));
        }
        let relative = scale(sub(*position, bounds.center), 1.0 / bounds.diagonal);
        if dot(relative, relative) > 4.0 {
            return Err(CoreError::OperationUnavailable(
                "Remesh worker output escaped the source mesh bounds.".into(),
            ));
        }
    }
    for index in &response.indices {
        if *index as usize >= response.positions.len() {
            return Err(CoreError::OperationUnavailable(
                "Remesh worker returned an out-of-range index.".into(),
            ));
        }
    }

    let mut edge_faces = std::collections::HashMap::<(u32, u32), (u8, i8)>::new();
    let mut unique_faces = std::collections::HashSet::<[u32; 3]>::new();
    let inverse_diagonal = 1.0 / bounds.diagonal;
    for triangle in response.indices.chunks_exact(3) {
        let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
        if a == b || b == c || c == a {
            return Err(CoreError::OperationUnavailable(
                "Remesh worker returned a degenerate triangle.".into(),
            ));
        }
        let mut face_key = [a, b, c];
        face_key.sort_unstable();
        if !unique_faces.insert(face_key) {
            return Err(CoreError::OperationUnavailable(
                "Remesh worker returned duplicate triangles.".into(),
            ));
        }
        let pa = response.positions[a as usize];
        let pb = response.positions[b as usize];
        let pc = response.positions[c as usize];
        let edge_a = scale(sub(pb, pa), inverse_diagonal);
        let edge_b = scale(sub(pc, pa), inverse_diagonal);
        let cross_product = cross(edge_a, edge_b);
        if !dot(cross_product, cross_product).is_finite() || dot(cross_product, cross_product) <= 1.0e-16 {
            return Err(CoreError::OperationUnavailable(
                "Remesh worker returned a zero-area or near-degenerate triangle.".into(),
            ));
        }
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let key = if u < v { (u, v) } else { (v, u) };
            let (count, winding) = edge_faces.entry(key).or_default();
            *count = count.saturating_add(1);
            *winding += if u < v { 1 } else { -1 };
            if *count > 2 {
                return Err(CoreError::OperationUnavailable(
                    "Remesh worker returned a non-manifold edge.".into(),
                ));
            }
        }
    }
    if edge_faces.values().any(|(count, _)| *count != 2) {
        return Err(CoreError::OperationUnavailable(
            "Remesh worker returned an open or non-watertight mesh.".into(),
        ));
    }
    if edge_faces.values().any(|(_, winding)| *winding != 0) {
        return Err(CoreError::OperationUnavailable(
            "Remesh worker returned inconsistently oriented faces.".into(),
        ));
    }
    Ok(())
}

fn geometry_bounds(positions: &[[f32; 3]]) -> CoreResult<Bounds> {
    if positions.is_empty() {
        return Err(CoreError::InvalidInput("Mesh positions are empty.".into()));
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    let span = sub(max, min);
    let center = [
        min[0] + span[0] * 0.5,
        min[1] + span[1] * 0.5,
        min[2] + span[2] * 0.5,
    ];
    let diagonal = dot(span, span).sqrt();
    if !diagonal.is_finite() || diagonal <= 1.0e-12 {
        return Err(CoreError::InvalidInput(
            "Mesh bounds are degenerate.".into(),
        ));
    }
    Ok(Bounds { center, diagonal })
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], scalar: f32) -> [f32; 3] {
    [a[0] * scalar, a[1] * scalar, a[2] * scalar]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_request() -> RemeshRequest {
        RemeshRequest {
            protocol_version: REMESH_PROTOCOL_VERSION,
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            indices: vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3],
            options: RetopologyOptions::default(),
        }
    }

    #[test]
    fn validates_finite_parameters_and_indexed_geometry() {
        assert!(validate_remesh_request(&valid_request()).is_ok());
        let mut bad = valid_request();
        bad.options.target_edge_ratio = f32::NAN;
        assert!(validate_remesh_request(&bad).is_err());
        let mut bad = valid_request();
        bad.indices[2] = 99;
        assert!(validate_remesh_request(&bad).is_err());
    }

    #[test]
    fn rejects_non_manifold_edges_before_spawning_worker() {
        let mut request = valid_request();
        request.positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, -1.0, 0.0],
        ];
        request.indices = vec![0, 1, 2, 1, 0, 3, 0, 1, 4];
        assert!(validate_remesh_request(&request).is_err());
    }

    #[test]
    fn rejects_duplicate_input_faces() {
        let mut request = valid_request();
        request.indices = vec![0, 1, 2, 0, 2, 1];
        assert!(validate_remesh_request(&request).is_err());
    }

    #[test]
    fn rejects_duplicate_degenerate_and_non_manifold_worker_output() {
        let request = valid_request();
        let duplicated = RemeshResponse {
            positions: request.positions.clone(),
            indices: vec![0, 1, 2, 0, 2, 1],
        };
        assert!(validate_remesh_response(&duplicated, &request).is_err());

        let degenerate = RemeshResponse {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            indices: vec![0, 1, 2],
        };
        assert!(validate_remesh_response(&degenerate, &request).is_err());

        let non_manifold = RemeshResponse {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, -1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            indices: vec![0, 1, 2, 1, 0, 3, 0, 1, 4],
        };
        assert!(validate_remesh_response(&non_manifold, &request).is_err());
    }

    #[test]
    fn target_edge_ratio_is_a_fraction_of_bounds() {
        let request = valid_request();
        let bounds = geometry_bounds(&request.positions).unwrap();
        assert!((bounds.diagonal - 3.0f32.sqrt()).abs() < 1.0e-6);
        assert!(
            (bounds.diagonal * request.options.target_edge_ratio
                - 0.05 * 3.0f32.sqrt())
                .abs()
                < 1.0e-6
        );
        let (normalized, _, factor) = normalize_remesh_positions(&request).unwrap();
        assert!((factor * request.options.target_edge_ratio * bounds.diagonal - 1.0).abs() < 1.0e-6);
        assert!(normalized.iter().all(|point| point.iter().all(|x| x.is_finite())));
    }
}
