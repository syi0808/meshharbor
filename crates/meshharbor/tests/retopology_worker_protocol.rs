#![cfg(feature = "experimental-retopology-worker")]

use std::{fs, path::PathBuf};

use meshharbor_core::{RetopologyOptions, inspect_asset, retopology_glb};

struct WorkerSidecar(PathBuf);

impl Drop for WorkerSidecar {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn tetrahedron_glb() -> Vec<u8> {
    let positions = [
        [0.0f32, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
    ];
    let indices = [0u16, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];
    let mut binary = Vec::new();
    for position in positions {
        for component in position {
            binary.extend_from_slice(&component.to_le_bytes());
        }
    }
    let index_offset = binary.len();
    for index in indices {
        binary.extend_from_slice(&index.to_le_bytes());
    }
    let document = serde_json::json!({
        "asset": { "version": "2.0" },
        "buffers": [{ "byteLength": binary.len() }],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": 48, "byteStride": 12, "target": 34962 },
            { "buffer": 0, "byteOffset": index_offset, "byteLength": 24, "target": 34963 }
        ],
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 1.0] },
            { "bufferView": 1, "componentType": 5123, "count": 12, "type": "SCALAR", "min": [0], "max": [3] }
        ],
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1, "mode": 4 }] }],
        "nodes": [{ "mesh": 0 }],
        "scenes": [{ "nodes": [0] }],
        "scene": 0
    });
    let mut json = serde_json::to_vec(&document).expect("encode GLB JSON");
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + binary.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x4e4f534au32.to_le_bytes());
    glb.extend_from_slice(&json);
    glb.extend_from_slice(&(binary.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x004e4942u32.to_le_bytes());
    glb.extend_from_slice(&binary);
    glb
}

fn glb_document(bytes: &[u8]) -> serde_json::Value {
    let json_length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&bytes[20..20 + json_length]).expect("decode output GLB JSON")
}

#[test]
fn glb_adapter_runs_the_real_worker_and_emits_validated_mesh() {
    let worker_source = PathBuf::from(env!("CARGO_BIN_EXE_meshharbor-retopo-worker"));
    let test_executable = std::env::current_exe().expect("test executable path");
    let worker_sidecar = test_executable.with_file_name(format!(
        "meshharbor-retopo-worker{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        !worker_sidecar.exists(),
        "worker sidecar should not overwrite an existing file"
    );
    fs::copy(worker_source, &worker_sidecar).expect("copy worker beside test harness");
    let _cleanup = WorkerSidecar(worker_sidecar);

    let (output, report) = retopology_glb(
        &tetrahedron_glb(),
        RetopologyOptions {
            target_edge_ratio: 0.2,
            iterations: 1,
        },
    )
    .expect("closed untextured tetrahedron should remesh");

    let descriptor = inspect_asset("retopo.glb", &output).expect("inspect output GLB");
    assert_eq!(descriptor.metadata["triangles"], report.output_triangles);
    let document = glb_document(&output);
    let attributes = document["meshes"][0]["primitives"][0]["attributes"]
        .as_object()
        .expect("output attributes");
    assert!(attributes.contains_key("POSITION"));
    assert!(attributes.contains_key("NORMAL"));
    assert_eq!(attributes.len(), 2);
    assert_eq!(report.input_triangles, 4);
    assert!(report.output_triangles > 0);
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("untextured")),
        "report should disclose the GLB support limit"
    );
}
