use std::{collections::HashMap, mem::size_of, ptr};

use serde::{Deserialize, Serialize};

use crate::{
    CoreError, CoreResult,
    glb::{encode_position_only_glb, extract_voxel_remesh_geometry},
    job_execution::JobExecution,
    quadriflow::{MAX_QUADRIFLOW_INPUT_GLB_BYTES, quadriflow_validation_request},
    retopology::MAX_TRIANGLES,
    validate_remesh_request,
};

pub const MIN_VOXEL_RESOLUTION: u32 = 16;
pub const MAX_VOXEL_RESOLUTION: u32 = 160;
const MAX_RASTER_SAMPLES: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct VoxelRemeshOptions {
    pub(crate) resolution: u32,
}

impl Default for VoxelRemeshOptions {
    fn default() -> Self {
        Self { resolution: 96 }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct VoxelRemeshReport {
    resolution: u32,
    input_triangles: usize,
    output_triangles: usize,
    output_vertices: usize,
    warnings: Vec<&'static str>,
}

pub(crate) fn validate_voxel_source_size(bytes: u64) -> CoreResult<()> {
    if bytes > MAX_QUADRIFLOW_INPUT_GLB_BYTES as u64 {
        return Err(CoreError::OperationUnavailable(
            "Voxel Remesh accepts GLB files up to 64 MiB.".into(),
        ));
    }
    Ok(())
}

pub(crate) fn voxel_remesh_glb(
    bytes: &[u8],
    options: VoxelRemeshOptions,
    execution: JobExecution<'_>,
) -> CoreResult<(Vec<u8>, VoxelRemeshReport)> {
    let resolution = options.resolution;
    if !(MIN_VOXEL_RESOLUTION..=MAX_VOXEL_RESOLUTION).contains(&resolution) {
        return Err(CoreError::InvalidInput(format!(
            "Voxel resolution must be between {MIN_VOXEL_RESOLUTION} and {MAX_VOXEL_RESOLUTION}."
        )));
    }
    validate_voxel_source_size(bytes.len() as u64)?;
    let (positions, mut indices) = extract_voxel_remesh_geometry(bytes)?;
    let input_triangles = indices.len() / 3;
    let mut positions = meshopt::optimize_vertex_fetch(&mut indices, &positions);
    let (center, extent) = normalize_positions(&mut positions)?;
    validate_raster_budget(&positions, &indices, resolution)?;
    execution.stage("voxelize_mesh")?;
    let corners = remesh_corners(&positions, &indices, resolution, execution)?;

    execution.stage("validate_voxel_mesh")?;
    let mut vertices = Vec::new();
    let mut indices = Vec::with_capacity(corners.len());
    let mut by_position = HashMap::new();
    for corner in corners {
        let restored: [f32; 3] =
            std::array::from_fn(|axis| (f64::from(corner[axis]) * extent + center[axis]) as f32);
        if restored.iter().any(|coordinate| !coordinate.is_finite()) {
            return Err(CoreError::OperationUnavailable(
                "Voxel Remesh produced a non-finite vertex.".into(),
            ));
        }
        let key = restored.map(|value| if value == 0.0 { 0 } else { value.to_bits() });
        let index = *by_position.entry(key).or_insert_with(|| {
            let index = vertices.len() as u32;
            vertices.push(restored);
            index
        });
        indices.push(index);
    }
    let rebuilt = quadriflow_validation_request(vertices, indices);
    validate_remesh_request(&rebuilt).map_err(|error| {
        CoreError::OperationUnavailable(format!(
            "Voxel Remesh could not produce valid closed geometry at resolution {resolution}: {error} Try a different voxel resolution."
        ))
    })?;
    let report = VoxelRemeshReport {
        resolution,
        input_triangles,
        output_triangles: rebuilt.indices.len() / 3,
        output_vertices: rebuilt.positions.len(),
        warnings: vec![
            "The derived GLB contains geometry only. UVs, materials, and textures remain on the source.",
            "Voxel reconstruction can merge close surfaces or lose thin detail. Topology validation does not certify self-intersection freedom or shape accuracy.",
        ],
    };
    Ok((
        encode_position_only_glb(&rebuilt.positions, &rebuilt.indices)?,
        report,
    ))
}

fn normalize_positions(positions: &mut [[f32; 3]]) -> CoreResult<([f64; 3], f64)> {
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for position in &*positions {
        for (axis, coordinate) in position.iter().copied().map(f64::from).enumerate() {
            minimum[axis] = minimum[axis].min(coordinate);
            maximum[axis] = maximum[axis].max(coordinate);
        }
    }
    let extent = (0..3)
        .map(|axis| maximum[axis] - minimum[axis])
        .fold(0.0, f64::max);
    if !extent.is_finite() || extent <= 0.0 {
        return Err(CoreError::InvalidInput(
            "Voxel Remesh requires nonzero mesh bounds.".into(),
        ));
    }
    let center = std::array::from_fn(|axis| (minimum[axis] + maximum[axis]) * 0.5);
    for position in positions {
        for axis in 0..3 {
            position[axis] = ((f64::from(position[axis]) - center[axis]) / extent) as f32;
        }
    }
    Ok((center, extent))
}

fn validate_raster_budget(
    positions: &[[f32; 3]],
    indices: &[u32],
    resolution: u32,
) -> CoreResult<()> {
    let mut samples = 0;
    for face in indices.chunks_exact(3) {
        let longest_edge_squared = [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])]
            .into_iter()
            .map(|(start, end)| {
                positions[start as usize]
                    .iter()
                    .zip(positions[end as usize])
                    .map(|(left, right)| (left - right).powi(2))
                    .sum::<f32>()
            })
            .fold(0.0, f32::max);
        let edge_samples = ((longest_edge_squared.sqrt() * (2 * resolution) as f32).ceil() as u64)
            .clamp(1, u64::from(2 * resolution));
        samples += (edge_samples + 1) * (edge_samples + 2) / 2;
        if samples > MAX_RASTER_SAMPLES {
            return Err(CoreError::OperationUnavailable(
                "Voxel Remesh exceeds the rasterization budget. Use a lower voxel resolution."
                    .into(),
            ));
        }
    }
    Ok(())
}

fn remesh_corners(
    positions: &[[f32; 3]],
    indices: &[u32],
    resolution: u32,
    execution: JobExecution<'_>,
) -> CoreResult<Vec<[f32; 3]>> {
    let remesh = |destination: *mut f32, max_triangles| unsafe {
        meshopt::ffi::meshopt_remesh(
            destination,
            max_triangles,
            indices.as_ptr(),
            indices.len(),
            positions.as_ptr().cast(),
            positions.len(),
            size_of::<[f32; 3]>(),
            resolution as i32,
            meshopt::RemeshOptions::Solve.bits(),
        )
    };
    let capacity = remesh(ptr::null_mut(), 0);
    execution.checkpoint()?;
    if capacity == 0 {
        return Err(CoreError::OperationUnavailable(
            "Voxel Remesh could not reconstruct any triangles.".into(),
        ));
    }
    if capacity > MAX_TRIANGLES {
        return Err(CoreError::OperationUnavailable(format!(
            "Voxel Remesh would exceed the {MAX_TRIANGLES}-triangle output budget. Use a lower voxel resolution."
        )));
    }
    let mut corners = vec![[0.0f32; 3]; capacity * 3];
    let actual = remesh(corners.as_mut_ptr().cast(), capacity);
    if actual == 0 || actual > capacity {
        return Err(CoreError::OperationUnavailable(
            "Voxel Remesh returned an invalid triangle count.".into(),
        ));
    }
    corners.truncate(actual * 3);
    Ok(corners)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QuadRetopologyOptions, quadriflow_preflight_glb};

    fn duplicate_cube() -> Vec<u8> {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for line in include_str!("../tests/fixtures/quadriflow_cube_triangles.obj").lines() {
            let mut fields = line.split_whitespace();
            match fields.next() {
                Some("v") => positions.push(std::array::from_fn(|_| {
                    fields.next().unwrap().parse().unwrap()
                })),
                Some("f") => indices.extend(fields.map(|index| index.parse::<u32>().unwrap() - 1)),
                _ => {}
            }
        }
        indices.extend([indices[0], indices[2], indices[1]]);
        encode_position_only_glb(&positions, &indices).unwrap()
    }

    #[test]
    fn voxel_remesh_rebuilds_duplicate_input_for_quad_preflight() {
        let source = duplicate_cube();
        assert!(quadriflow_preflight_glb(&source, QuadRetopologyOptions::default()).is_err());

        let (result, report) = voxel_remesh_glb(
            &source,
            VoxelRemeshOptions { resolution: 24 },
            JobExecution::default(),
        )
        .unwrap();
        let checked = quadriflow_preflight_glb(&result, QuadRetopologyOptions::default()).unwrap();
        assert_eq!(report.input_triangles, 13);
        assert_eq!(checked.input_triangles, report.output_triangles as u64);
        assert_eq!(checked.input_vertices, report.output_vertices as u64);
        assert!(!checked.appearance_projection_required);
    }

    #[test]
    fn voxel_inputs_and_work_budgets_are_bounded() {
        for resolution in [MIN_VOXEL_RESOLUTION - 1, MAX_VOXEL_RESOLUTION + 1] {
            assert!(matches!(
                voxel_remesh_glb(
                    &[],
                    VoxelRemeshOptions { resolution },
                    JobExecution::default()
                ),
                Err(CoreError::InvalidInput(_))
            ));
        }
        let positions = [[-0.5, -0.5, -0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5]];
        let indices = [0, 1, 2].repeat(5_000);
        assert!(validate_raster_budget(&positions, &indices, MAX_VOXEL_RESOLUTION).is_err());
    }
}
