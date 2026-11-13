use super::{AtlasBakeError, AtlasBakeOptions, bake_glb};
use serde_json::{Value, json};

const BASE_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0,
    0, 0, 114, 182, 13, 36, 0, 0, 0, 20, 73, 68, 65, 84, 120, 218, 99, 252, 47, 199, 245, 159, 129,
    129, 129, 129, 137, 1, 10, 0, 33, 141, 2, 42, 48, 155, 50, 207, 0, 0, 0, 0, 73, 69, 78, 68,
    174, 66, 96, 130,
];
const MR_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0,
    0, 0, 114, 182, 13, 36, 0, 0, 0, 20, 73, 68, 65, 84, 120, 218, 99, 100, 104, 248, 255, 159,
    129, 129, 129, 129, 137, 1, 10, 0, 37, 25, 2, 130, 79, 188, 53, 61, 0, 0, 0, 0, 73, 69, 78, 68,
    174, 66, 96, 130,
];

fn append_view(
    binary: &mut Vec<u8>,
    views: &mut Vec<Value>,
    bytes: &[u8],
    target: Option<u64>,
) -> usize {
    while binary.len() % 4 != 0 {
        binary.push(0);
    }
    let offset = binary.len();
    binary.extend_from_slice(bytes);
    let mut view = json!({ "buffer": 0, "byteOffset": offset, "byteLength": bytes.len() });
    if let Some(target) = target {
        view["target"] = json!(target);
    }
    let index = views.len();
    views.push(view);
    index
}

fn floats(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn fixture(with_tangent: bool, with_normal_map: bool, with_color: bool) -> Vec<u8> {
    let mut binary = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let positions = [
        0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0,
    ];
    let normals = [
        0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0,
    ];
    let uv = [0.0f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
    let pos_view = append_view(&mut binary, &mut views, &floats(&positions), Some(34962));
    accessors.push(json!({"bufferView":pos_view,"componentType":5126,"count":4,"type":"VEC3"}));
    let normal_view = append_view(&mut binary, &mut views, &floats(&normals), Some(34962));
    accessors.push(json!({"bufferView":normal_view,"componentType":5126,"count":4,"type":"VEC3"}));
    let uv_view = append_view(&mut binary, &mut views, &floats(&uv), Some(34962));
    accessors.push(json!({"bufferView":uv_view,"componentType":5126,"count":4,"type":"VEC2"}));
    let index_bytes: Vec<u8> = [0u16, 1, 2, 0, 2, 3]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    let index_view = append_view(&mut binary, &mut views, &index_bytes, Some(34963));
    accessors.push(json!({"bufferView":index_view,"componentType":5123,"count":6,"type":"SCALAR"}));
    let tangent_accessor = if with_tangent {
        let tangent = [
            1.0f32, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0,
        ];
        let view = append_view(&mut binary, &mut views, &floats(&tangent), Some(34962));
        accessors.push(json!({"bufferView":view,"componentType":5126,"count":4,"type":"VEC4"}));
        Some(accessors.len() - 1)
    } else {
        None
    };
    let color_accessor = if with_color {
        let color = [
            1.0f32, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
        ];
        let view = append_view(&mut binary, &mut views, &floats(&color), Some(34962));
        accessors.push(json!({"bufferView":view,"componentType":5126,"count":4,"type":"VEC4"}));
        Some(accessors.len() - 1)
    } else {
        None
    };
    let base_view = append_view(&mut binary, &mut views, BASE_PNG, None);
    let mr_view = append_view(&mut binary, &mut views, MR_PNG, None);
    let mut attributes = json!({"POSITION":0,"NORMAL":1,"TEXCOORD_0":2});
    if let Some(index) = tangent_accessor {
        attributes["TANGENT"] = json!(index);
    }
    if let Some(index) = color_accessor {
        attributes["COLOR_0"] = json!(index);
    }
    let mut material = json!({"pbrMetallicRoughness": {
        "baseColorTexture": {"index":0},
        "metallicRoughnessTexture": {"index":1}
    }});
    if with_normal_map {
        material["normalTexture"] = json!({"index": 99});
    }
    let document = json!({
        "asset":{"version":"2.0"},
        "buffers":[{"byteLength":binary.len()}],
        "bufferViews":views,
        "accessors":accessors,
        "images":[{"bufferView":base_view,"mimeType":"image/png"},{"bufferView":mr_view,"mimeType":"image/png"}],
        "textures":[{"source":0},{"source":1}],
        "materials":[material],
        "meshes":[{"primitives":[{"attributes":attributes,"indices":3,"material":0}]}],
        "nodes":[{"mesh":0}],
        "scenes":[{"nodes":[0]}],
        "scene":0
    });
    super::Glb {
        document,
        binary,
        other_chunks: Vec::new(),
    }
    .write()
    .unwrap()
}

#[test]
fn atlas_rebakes_textures_and_preserves_non_uv_vertex_streams() {
    let input = fixture(false, false, true);
    let (output, report) = bake_glb(
        &input,
        AtlasBakeOptions {
            resolution: 128,
            padding: 4,
        },
    )
    .unwrap();
    assert_eq!(report.source_triangles, 2);
    assert_eq!(report.output_triangles, 2);
    assert_eq!(report.chart_count, 1);
    assert!(report.preserved_attributes.contains(&"COLOR_0".to_owned()));
    assert!(report.covered_texels > 0);

    let glb = super::Glb::parse(&output).unwrap();
    assert_eq!(glb.document["images"].as_array().unwrap().len(), 4);
    assert_eq!(glb.document["textures"].as_array().unwrap().len(), 4);
    let primitive = &glb.document["meshes"][0]["primitives"][0];
    let color_accessor = primitive["attributes"]["COLOR_0"].as_u64().unwrap() as usize;
    let colors = super::read_accessor(&glb.document, &glb.binary, color_accessor).unwrap();
    assert_eq!(colors.count, report.output_vertices as usize);
    let expected = floats(&[
        1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    ]);
    let color_values: Vec<[u8; 16]> = colors
        .bytes
        .chunks_exact(16)
        .map(|bytes| bytes.try_into().unwrap())
        .collect();
    let expected_values: Vec<[u8; 16]> = expected
        .chunks_exact(16)
        .map(|bytes| bytes.try_into().unwrap())
        .collect();
    assert_eq!(color_values.len(), 4);
    for color in color_values {
        assert!(expected_values.contains(&color));
    }

    let base_texture = primitive["material"].as_u64().unwrap() as usize;
    assert_eq!(base_texture, 0);
    let material = &glb.document["materials"][0];
    let new_base_texture = material["pbrMetallicRoughness"]["baseColorTexture"]["index"]
        .as_u64()
        .unwrap() as usize;
    let new_base_image = glb.document["textures"][new_base_texture]["source"]
        .as_u64()
        .unwrap() as usize;
    let new_base_view = glb.document["images"][new_base_image]["bufferView"]
        .as_u64()
        .unwrap() as usize;
    let (start, length, _) = super::view_range(&glb.document, &glb.binary, new_base_view).unwrap();
    assert!(glb.binary[start..start + length].starts_with(b"\x89PNG\r\n\x1a\n"));
}

#[test]
fn refuses_stale_tangents_and_unbaked_material_maps() {
    assert!(matches!(
        bake_glb(&fixture(true, false, false), AtlasBakeOptions::default()),
        Err(AtlasBakeError::Unsupported(_))
    ));
    assert!(matches!(
        bake_glb(&fixture(false, true, false), AtlasBakeOptions::default()),
        Err(AtlasBakeError::Unsupported(_))
    ));
}

#[test]
fn refuses_unsafe_resolution_and_malformed_glb() {
    assert!(matches!(
        bake_glb(
            &fixture(false, false, false),
            AtlasBakeOptions {
                resolution: 8192,
                padding: 4
            }
        ),
        Err(AtlasBakeError::InvalidInput(_))
    ));
    assert!(matches!(
        bake_glb(b"not a GLB", AtlasBakeOptions::default()),
        Err(AtlasBakeError::InvalidInput(_))
    ));
}

#[test]
fn atlas_options_deserialize_to_safe_defaults_and_reject_unknown_fields() {
    let options: AtlasBakeOptions = serde_json::from_value(json!({})).unwrap();
    assert_eq!(options, AtlasBakeOptions::default());
    assert!(serde_json::from_value::<AtlasBakeOptions>(json!({"quality": 2})).is_err());
}
