use serde_json::{Value, json};

use super::{GlbFile, MAX_ASSET_BYTES, array, validate_asset_size};
use crate::CoreError;

const COMPRESSION: [&str; 2] = ["KHR_meshopt_compression", "EXT_meshopt_compression"];

pub fn prepare_blender_glb(bytes: Vec<u8>) -> Result<Vec<u8>, CoreError> {
    validate_asset_size(bytes.len())?;
    let mut glb = GlbFile::parse(&bytes)?;
    for collection in ["buffers", "images"] {
        for entry in array(&glb.document, collection) {
            if entry
                .get("uri")
                .is_some_and(|uri| !uri.as_str().is_some_and(|uri| uri.starts_with("data:")))
            {
                return Err(CoreError::OperationUnavailable(
                    "Blender transfers require embedded buffers and textures; external GLB references are not supported.".into(),
                ));
            }
        }
    }
    let compressed = array(&glb.document, "bufferViews").iter().any(|view| {
        COMPRESSION
            .iter()
            .any(|name| view["extensions"].get(*name).is_some())
    });
    if !compressed {
        return Ok(bytes);
    }
    drop(bytes);
    let buffers = array(&glb.document, "buffers");
    let binary = glb
        .binary
        .take()
        .and_then(|binary| {
            let size = number(buffers.first()?, "byteLength")?;
            (size <= binary.len() && binary.len() - size <= 3).then_some((binary, size))
        })
        .ok_or_else(invalid_compression)?;
    if buffers
        .first()
        .is_none_or(|buffer| buffer.get("uri").is_some())
        || buffers.iter().skip(1).any(|buffer| {
            !COMPRESSION
                .iter()
                .any(|name| buffer["extensions"][*name]["fallback"] == true)
                && buffer.get("uri").is_some()
        })
    {
        return Err(invalid_compression());
    }
    let binary = &binary.0[..binary.1];
    let mut decoded = Vec::new();
    for view in glb.document["bufferViews"]
        .as_array_mut()
        .ok_or_else(invalid_compression)?
    {
        let length = number(view, "byteLength").ok_or_else(invalid_compression)?;
        let offset = decoded.len().next_multiple_of(4);
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= MAX_ASSET_BYTES)
            .ok_or_else(expansion_limit)?;
        decoded.resize(offset, 0);
        if let Some(extension) = COMPRESSION
            .iter()
            .find_map(|name| view["extensions"].get(*name))
        {
            decode_view(extension, binary, length, &mut decoded)?;
            if let Some(extensions) = view["extensions"].as_object_mut() {
                for name in COMPRESSION {
                    extensions.remove(name);
                }
                if extensions.is_empty() {
                    view.as_object_mut()
                        .ok_or_else(invalid_compression)?
                        .remove("extensions");
                }
            }
        } else {
            decoded.extend_from_slice(embedded_slice(view, binary)?);
        }
        debug_assert_eq!(decoded.len(), end);
        view["buffer"] = json!(0);
        view["byteOffset"] = json!(offset);
    }
    glb.document["buffers"] = json!([{"byteLength": decoded.len()}]);
    for list in ["extensionsUsed", "extensionsRequired"] {
        if let Some(names) = glb.document[list].as_array_mut() {
            names.retain(|name| {
                !name
                    .as_str()
                    .is_some_and(|name| COMPRESSION.contains(&name))
            });
            if names.is_empty() {
                glb.document
                    .as_object_mut()
                    .ok_or_else(invalid_compression)?
                    .remove(list);
            }
        }
    }
    glb.binary = Some(decoded);
    let output = glb.write()?;
    if output.len() > MAX_ASSET_BYTES {
        return Err(expansion_limit());
    }
    Ok(output)
}

fn decode_view(
    extension: &Value,
    binary: &[u8],
    length: usize,
    output: &mut Vec<u8>,
) -> Result<(), CoreError> {
    let source = embedded_slice(extension, binary)?;
    let count = number(extension, "count").ok_or_else(invalid_compression)?;
    let stride = number(extension, "byteStride").ok_or_else(invalid_compression)?;
    if count == 0 || count.checked_mul(stride) != Some(length) || source.is_empty() {
        return Err(invalid_compression());
    }
    let mode = extension["mode"].as_str().ok_or_else(invalid_compression)?;
    let filter = match extension.get("filter") {
        None => "NONE",
        Some(value) => value.as_str().ok_or_else(invalid_compression)?,
    };
    match (mode, filter) {
        ("ATTRIBUTES", "NONE") if (4..=256).contains(&stride) && stride % 4 == 0 => {}
        ("ATTRIBUTES", "OCTAHEDRAL" | "COLOR") if matches!(stride, 4 | 8) => {}
        ("ATTRIBUTES", "QUATERNION") if stride == 8 => {}
        ("ATTRIBUTES", "EXPONENTIAL") if (4..=256).contains(&stride) && stride % 4 == 0 => {}
        ("TRIANGLES", "NONE") if count % 3 == 0 && matches!(stride, 2 | 4) => {}
        ("INDICES", "NONE") if matches!(stride, 2 | 4) => {}
        _ => return Err(invalid_compression()),
    }
    let mut words = vec![0u32; length.div_ceil(4)];
    let destination = words.as_mut_ptr().cast();
    let result = unsafe {
        match mode {
            "ATTRIBUTES" => meshopt::ffi::meshopt_decodeVertexBuffer(
                destination,
                count,
                stride,
                source.as_ptr(),
                source.len(),
            ),
            "TRIANGLES" => meshopt::ffi::meshopt_decodeIndexBuffer(
                destination,
                count,
                stride,
                source.as_ptr(),
                source.len(),
            ),
            "INDICES" => meshopt::ffi::meshopt_decodeIndexSequence(
                destination,
                count,
                stride,
                source.as_ptr(),
                source.len(),
            ),
            _ => unreachable!(),
        }
    };
    if result != 0 {
        return Err(invalid_compression());
    }
    unsafe {
        match filter {
            "OCTAHEDRAL" => meshopt::ffi::meshopt_decodeFilterOct(destination, count, stride),
            "QUATERNION" => meshopt::ffi::meshopt_decodeFilterQuat(destination, count, stride),
            "EXPONENTIAL" => meshopt::ffi::meshopt_decodeFilterExp(destination, count, stride),
            "COLOR" => meshopt::ffi::meshopt_decodeFilterColor(destination, count, stride),
            _ => {}
        }
        output.extend_from_slice(std::slice::from_raw_parts(destination.cast::<u8>(), length));
    }
    Ok(())
}

fn number(object: &Value, key: &str) -> Option<usize> {
    usize::try_from(object.get(key)?.as_u64()?).ok()
}

fn embedded_slice<'a>(view: &Value, binary: &'a [u8]) -> Result<&'a [u8], CoreError> {
    if number(view, "buffer") != Some(0) {
        return Err(invalid_compression());
    }
    let start = match view.get("byteOffset") {
        Some(_) => number(view, "byteOffset").ok_or_else(invalid_compression)?,
        None => 0,
    };
    let length = number(view, "byteLength").ok_or_else(invalid_compression)?;
    start
        .checked_add(length)
        .and_then(|end| binary.get(start..end))
        .ok_or_else(invalid_compression)
}

fn invalid_compression() -> CoreError {
    CoreError::InvalidInput("This GLB has an invalid or unsupported meshopt buffer layout.".into())
}

fn expansion_limit() -> CoreError {
    CoreError::OperationUnavailable("The Blender-compatible GLB would exceed 512 MiB.".into())
}
