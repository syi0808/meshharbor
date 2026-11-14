//! Narrow safe wrapper around the vendored xatlas + stb CPU UV/texture baker.

use std::{ffi::c_char, ptr};

#[derive(Debug, Clone)]
pub struct AtlasResult {
    pub uv: Vec<[f32; 2]>,
    pub xref: Vec<u32>,
    pub indices: Vec<u32>,
    pub width: u32,
    pub height: u32,
    pub chart_count: u32,
    pub base_color_png: Vec<u8>,
    pub metallic_roughness_png: Vec<u8>,
    pub covered_texels: u64,
    pub gutter_texels: u64,
    pub projection: Option<ProjectionStats>,
}

/// Coverage and distance bounds for a geometry-to-geometry texture projection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectionStats {
    pub attempted_texels: u64,
    pub projected_texels: u64,
    pub ambiguous_texels: u64,
    pub maximum_distance: f32,
    pub distance_limit: f32,
}

#[repr(C)]
struct RawResult {
    vertex_count: u32,
    index_count: u32,
    width: u32,
    height: u32,
    chart_count: u32,
    uv: *mut f32,
    xref: *mut u32,
    indices: *mut u32,
    base_png: *mut u8,
    base_png_len: usize,
    mr_png: *mut u8,
    mr_png_len: usize,
    covered_texels: u64,
    gutter_texels: u64,
    projection_attempted: u64,
    projection_projected: u64,
    projection_ambiguous: u64,
    projection_max_distance: f32,
    projection_distance_limit: f32,
}

impl Default for RawResult {
    fn default() -> Self {
        Self {
            vertex_count: 0,
            index_count: 0,
            width: 0,
            height: 0,
            chart_count: 0,
            uv: ptr::null_mut(),
            xref: ptr::null_mut(),
            indices: ptr::null_mut(),
            base_png: ptr::null_mut(),
            base_png_len: 0,
            mr_png: ptr::null_mut(),
            mr_png_len: 0,
            covered_texels: 0,
            gutter_texels: 0,
            projection_attempted: 0,
            projection_projected: 0,
            projection_ambiguous: 0,
            projection_max_distance: 0.0,
            projection_distance_limit: 0.0,
        }
    }
}

unsafe extern "C" {
    fn meshharbor_atlas_bake(
        positions: *const f32,
        normals: *const f32,
        original_uv: *const f32,
        vertex_count: u32,
        indices: *const u32,
        index_count: u32,
        base_png: *const u8,
        base_png_len: usize,
        mr_png: *const u8,
        mr_png_len: usize,
        resolution: u32,
        padding: u32,
        source_positions: *const f32,
        source_uv: *const f32,
        source_vertex_count: u32,
        source_indices: *const u32,
        source_index_count: u32,
        max_distance_ratio: f32,
        min_coverage: f32,
        output: *mut RawResult,
        error: *mut c_char,
        error_capacity: usize,
    ) -> i32;
    fn meshharbor_atlas_free(output: *mut RawResult);
}

/// CPU data supplied to xatlas. The references are borrowed for one synchronous bake.
pub struct AtlasInput<'a> {
    pub positions: &'a [[f32; 3]],
    pub normals: Option<&'a [[f32; 3]]>,
    pub original_uv: &'a [[f32; 2]],
    pub indices: &'a [u32],
    pub base_png: &'a [u8],
    pub mr_png: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct AtlasSettings {
    pub resolution: u32,
    pub padding: u32,
}

/// Source geometry and UVs used to project a remeshed target onto original
/// appearance. The source image inputs remain the same embedded PNG pair.
pub struct ProjectionSource<'a> {
    pub positions: &'a [[f32; 3]],
    pub uv: &'a [[f32; 2]],
    pub indices: &'a [u32],
}

/// Generate an xatlas UV set and rebake an embedded glTF base-color and
/// metallic/roughness PNG pair. Geometry is not simplified by this crate.
pub fn unwrap_and_bake(
    input: AtlasInput<'_>,
    settings: AtlasSettings,
) -> Result<AtlasResult, String> {
    unwrap_and_bake_internal(input, settings, None)
}

/// Generate target atlas UVs and transfer embedded PBR textures from a source
/// surface. This is deliberately limited to bounded, close, similarly-facing
/// projections; it rejects ambiguous thin-shell samples and insufficient
/// texel coverage.
pub fn unwrap_and_bake_projected(
    input: AtlasInput<'_>,
    settings: AtlasSettings,
    source: ProjectionSource<'_>,
    max_distance_ratio: f32,
    min_coverage: f32,
) -> Result<AtlasResult, String> {
    unwrap_and_bake_internal(
        input,
        settings,
        Some((source, max_distance_ratio, min_coverage)),
    )
}

fn unwrap_and_bake_internal(
    input: AtlasInput<'_>,
    settings: AtlasSettings,
    projection: Option<(ProjectionSource<'_>, f32, f32)>,
) -> Result<AtlasResult, String> {
    let AtlasInput {
        positions,
        normals,
        original_uv,
        indices,
        base_png,
        mr_png,
    } = input;
    let AtlasSettings {
        resolution,
        padding,
    } = settings;
    if positions.is_empty()
        || projection.is_none() && positions.len() != original_uv.len()
        || projection.is_some() && !original_uv.is_empty()
    {
        return Err(
            "target positions must be non-empty; source UVs are only accepted for same-mesh baking"
                .into(),
        );
    }
    if positions.len() > 5_000_000 || indices.len() > 15_000_000 {
        return Err("mesh exceeds the native atlas safety limits".into());
    }
    if positions.iter().flatten().any(|value| !value.is_finite())
        || original_uv.iter().flatten().any(|value| !value.is_finite())
    {
        return Err("positions and TEXCOORD_0 must contain only finite values".into());
    }
    if let Some(normals) = normals {
        if normals.len() != positions.len() {
            return Err("NORMAL must have the same vertex count as POSITION".into());
        }
        if normals.iter().flatten().any(|value| !value.is_finite()) {
            return Err("NORMAL must contain only finite values".into());
        }
    }
    if projection.is_some() && normals.is_none() {
        return Err(
            "projected rebaking requires target normals for the facing-direction gate".into(),
        );
    }
    if indices.is_empty() || !indices.len().is_multiple_of(3) {
        return Err("indices must contain one or more complete triangles".into());
    }
    if indices
        .iter()
        .any(|index| *index as usize >= positions.len())
    {
        return Err("an index points outside the POSITION array".into());
    }
    if let Some((source, max_distance_ratio, min_coverage)) = &projection {
        if source.positions.is_empty()
            || source.positions.len() != source.uv.len()
            || source.indices.is_empty()
            || !source.indices.len().is_multiple_of(3)
            || source.indices.len() > 1_500_000
            || source.positions.len() > 5_000_000
            || source
                .positions
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
            || source.uv.iter().flatten().any(|value| !value.is_finite())
            || source
                .indices
                .iter()
                .any(|index| *index as usize >= source.positions.len())
        {
            return Err(
                "source projection mesh must have finite positions/UVs and bounded valid triangles"
                    .into(),
            );
        }
        if !max_distance_ratio.is_finite()
            || !(0.0001..=0.05).contains(max_distance_ratio)
            || !min_coverage.is_finite()
            || !(0.95..=1.0).contains(min_coverage)
        {
            return Err(
                "projection distance ratio must be 0.0001..0.05 and minimum coverage 0.95..1.0"
                    .into(),
            );
        }
    }
    if !(64..=4096).contains(&resolution)
        || padding > 64
        || u64::from(resolution) * u64::from(resolution) > 16_777_216
    {
        return Err("resolution must be 64..4096, the atlas may not exceed 16M texels, and padding must be 0..64".into());
    }
    if base_png.len() > 128 * 1024 * 1024
        || mr_png.len() > 128 * 1024 * 1024
        || !base_png.starts_with(b"\x89PNG\r\n\x1a\n")
        || !mr_png.starts_with(b"\x89PNG\r\n\x1a\n")
    {
        return Err("base-color and metallic-roughness inputs must be PNGs under 128 MiB".into());
    }
    let vertex_count =
        u32::try_from(positions.len()).map_err(|_| "too many vertices for xatlas")?;
    let index_count = u32::try_from(indices.len()).map_err(|_| "too many indices for xatlas")?;
    let flat_positions: Vec<f32> = positions.iter().flatten().copied().collect();
    let flat_normals: Vec<f32> = normals
        .map(|values| values.iter().flatten().copied().collect())
        .unwrap_or_default();
    let flat_uv: Vec<f32> = original_uv.iter().flatten().copied().collect();
    let flat_source_positions: Vec<f32> = projection
        .as_ref()
        .map(|(source, _, _)| source.positions.iter().flatten().copied().collect())
        .unwrap_or_default();
    let flat_source_uv: Vec<f32> = projection
        .as_ref()
        .map(|(source, _, _)| source.uv.iter().flatten().copied().collect())
        .unwrap_or_default();
    let (source_indices, max_distance_ratio, min_coverage) = projection
        .as_ref()
        .map(|(source, distance, coverage)| (source.indices, *distance, *coverage))
        .unwrap_or((&[][..], 0.0, 0.0));
    let mut raw = RawResult::default();
    let mut error = [0_i8; 1024];
    // SAFETY: all slices remain alive for the duration of the synchronous C++ call;
    // the returned buffers are owned by the native result and released below.
    let status = unsafe {
        meshharbor_atlas_bake(
            flat_positions.as_ptr(),
            if flat_normals.is_empty() {
                ptr::null()
            } else {
                flat_normals.as_ptr()
            },
            if flat_uv.is_empty() {
                ptr::null()
            } else {
                flat_uv.as_ptr()
            },
            vertex_count,
            indices.as_ptr(),
            index_count,
            base_png.as_ptr(),
            base_png.len(),
            mr_png.as_ptr(),
            mr_png.len(),
            resolution,
            padding,
            if flat_source_positions.is_empty() {
                ptr::null()
            } else {
                flat_source_positions.as_ptr()
            },
            if flat_source_uv.is_empty() {
                ptr::null()
            } else {
                flat_source_uv.as_ptr()
            },
            u32::try_from(flat_source_positions.len() / 3)
                .map_err(|_| "too many source vertices")?,
            if source_indices.is_empty() {
                ptr::null()
            } else {
                source_indices.as_ptr()
            },
            u32::try_from(source_indices.len()).map_err(|_| "too many source indices")?,
            max_distance_ratio,
            min_coverage,
            &mut raw,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if status != 0 {
        let message = unsafe {
            let bytes = std::slice::from_raw_parts(error.as_ptr().cast::<u8>(), error.len());
            let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        };
        unsafe { meshharbor_atlas_free(&mut raw) };
        return Err(if message.is_empty() {
            "xatlas CPU baker failed".into()
        } else {
            message
        });
    }
    if raw.vertex_count == 0
        || raw.index_count == 0
        || raw.uv.is_null()
        || raw.xref.is_null()
        || raw.indices.is_null()
        || raw.base_png.is_null()
        || raw.mr_png.is_null()
    {
        unsafe { meshharbor_atlas_free(&mut raw) };
        return Err("xatlas returned an incomplete native result".into());
    }
    let result = unsafe {
        let uv_flat = std::slice::from_raw_parts(raw.uv, raw.vertex_count as usize * 2);
        let uv = uv_flat
            .chunks_exact(2)
            .map(|pair| [pair[0], pair[1]])
            .collect();
        let xref = std::slice::from_raw_parts(raw.xref, raw.vertex_count as usize).to_vec();
        let indices = std::slice::from_raw_parts(raw.indices, raw.index_count as usize).to_vec();
        let base_color_png = std::slice::from_raw_parts(raw.base_png, raw.base_png_len).to_vec();
        let metallic_roughness_png =
            std::slice::from_raw_parts(raw.mr_png, raw.mr_png_len).to_vec();
        AtlasResult {
            uv,
            xref,
            indices,
            width: raw.width,
            height: raw.height,
            chart_count: raw.chart_count,
            base_color_png,
            metallic_roughness_png,
            covered_texels: raw.covered_texels,
            gutter_texels: raw.gutter_texels,
            projection: projection.as_ref().map(|_| ProjectionStats {
                attempted_texels: raw.projection_attempted,
                projected_texels: raw.projection_projected,
                ambiguous_texels: raw.projection_ambiguous,
                maximum_distance: raw.projection_max_distance,
                distance_limit: raw.projection_distance_limit,
            }),
        }
    };
    unsafe { meshharbor_atlas_free(&mut raw) };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{AtlasInput, AtlasSettings, unwrap_and_bake};

    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6,
        0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 20, 73, 68, 65, 84, 120, 218, 99, 252, 47, 199, 245,
        159, 129, 129, 129, 129, 137, 1, 10, 0, 33, 141, 2, 42, 48, 155, 50, 207, 0, 0, 0, 0, 73,
        69, 78, 68, 174, 66, 96, 130,
    ];

    #[test]
    fn disconnected_charts_get_separate_islands_and_same_chart_gutters() {
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [2.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [3.0, 1.0, 0.0],
        ];
        let normals = [[0.0, 0.0, 1.0]; 6];
        let uv = [
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
        ];
        let indices = [0, 1, 2, 3, 4, 5];
        let result = unwrap_and_bake(
            AtlasInput {
                positions: &positions,
                normals: Some(&normals),
                original_uv: &uv,
                indices: &indices,
                base_png: PNG,
                mr_png: PNG,
            },
            AtlasSettings {
                resolution: 128,
                padding: 8,
            },
        )
        .unwrap();
        assert_eq!(result.indices.len(), 6);
        assert_eq!(result.uv.len(), result.xref.len());
        assert!(result.chart_count >= 2);
        assert!(result.covered_texels > 0);
        assert!(result.gutter_texels > 0);
        assert!(result.base_color_png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(
            result
                .metallic_roughness_png
                .starts_with(b"\x89PNG\r\n\x1a\n")
        );
    }
}
