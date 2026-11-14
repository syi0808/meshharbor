use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[path = "build_support/runtime_bundle.rs"]
mod runtime_bundle;

const EXPECTED_RUNTIME_BUNDLE_MANIFEST_SHA256: &str =
    "4cacde8e03f4dcf31fd4a2d6d9614eda9b599b8be8f0d646c7f98f954db869a1";
const EXPECTED_RUNTIME_BUNDLE_FILE_COUNT: usize = 209;

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_root = manifest_dir.join("../..");
    let manifest_path = manifest_dir.join("runtime_bundle_manifest.json");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support/runtime_bundle.rs");

    let prepared = runtime_bundle::prepare_bundle(
        &workspace_root,
        &manifest_path,
        &out_dir,
        Some(EXPECTED_RUNTIME_BUNDLE_MANIFEST_SHA256),
        Some(EXPECTED_RUNTIME_BUNDLE_FILE_COUNT),
    )
    .unwrap_or_else(|error| panic!("runtime bundle verification failed: {error}"));

    for directory in prepared.watched_directories {
        emit_rerun_if_changed(&manifest_dir, &directory);
    }
    for file in prepared.watched_files {
        emit_rerun_if_changed(&manifest_dir, &file);
    }

    fs::write(
        out_dir.join("pixal3d_runtime_bundle.rs"),
        prepared.generated_source,
    )
    .expect("write verified embedded runtime manifest");
}

fn emit_rerun_if_changed(manifest_dir: &Path, path: &Path) {
    let relative = path
        .strip_prefix(manifest_dir)
        .expect("runtime bundle watch path is under the crate manifest directory");
    let relative = relative
        .to_str()
        .expect("runtime bundle watch paths must be UTF-8");
    if relative.chars().any(char::is_control) {
        panic!("runtime bundle watch path contains a control character");
    }
    println!("cargo:rerun-if-changed={relative}");
}
