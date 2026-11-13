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
        output: *mut RawResult,
        error: *mut c_char,
        error_capacity: usize,
    ) -> i32;
    fn meshharbor_atlas_free(output: *mut RawResult);
}

/// Generate an xatlas UV set and rebake an embedded glTF base-color and
/// metallic/roughness PNG pair. Geometry is not simplified by this crate.
pub fn unwrap_and_bake(
    positions: &[[f32; 3]],
    normals: Option<&[[f32; 3]]>,
    original_uv: &[[f32; 2]],
    indices: &[u32],
    base_png: &[u8],
    mr_png: &[u8],
    resolution: u32,
    padding: u32,
) -> Result<AtlasResult, String> {
    if positions.is_empty() || positions.len() != original_uv.len() {
        return Err("positions and TEXCOORD_0 must have the same non-zero vertex count".into());
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
    if indices.is_empty() || indices.len() % 3 != 0 {
        return Err("indices must contain one or more complete triangles".into());
    }
    if indices
        .iter()
        .any(|index| *index as usize >= positions.len())
    {
        return Err("an index points outside the POSITION array".into());
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
            flat_uv.as_ptr(),
            vertex_count,
            indices.as_ptr(),
            index_count,
            base_png.as_ptr(),
            base_png.len(),
            mr_png.as_ptr(),
            mr_png.len(),
            resolution,
            padding,
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
        }
    };
    unsafe { meshharbor_atlas_free(&mut raw) };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::unwrap_and_bake;

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
        let result =
            unwrap_and_bake(&positions, Some(&normals), &uv, &indices, PNG, PNG, 128, 8).unwrap();
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
