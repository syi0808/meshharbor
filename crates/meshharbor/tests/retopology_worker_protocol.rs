#![cfg(feature = "experimental-retopology-worker")]

use std::{
    fs,
    path::PathBuf,
};

use meshharbor_core::{RetopologyOptions, remesh_triangles_in_worker};

struct WorkerSidecar(PathBuf);

impl Drop for WorkerSidecar {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn core_adapter_runs_the_real_worker_as_a_child_process() {
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

    let positions = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
    ];
    let indices = [0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];
    let (response, report) = remesh_triangles_in_worker(
        &positions,
        &indices,
        RetopologyOptions {
            target_edge_ratio: 0.2,
            iterations: 1,
        },
    )
    .expect("worker child should remesh a closed tetrahedron");

    assert!(!response.positions.is_empty());
    assert!(!response.indices.is_empty());
    assert_eq!(response.indices.len() % 3, 0);
    assert_eq!(report.algorithm, "isotropic_triangle_remeshing");
    assert_eq!(report.input_triangles, 4);
    assert_eq!(report.output_triangles, (response.indices.len() / 3) as u64);
}
