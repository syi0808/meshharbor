use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[path = "build_support/studio_assets.rs"]
mod studio_assets;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let studio_dir = manifest_dir.join("../../apps/studio");
    let dist_dir = studio_dir.join("dist");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support/studio_assets.rs");
    for path in ["package.json", "package-lock.json", "vite.config.ts"] {
        emit_rerun_if_changed(&manifest_dir, &studio_dir.join(path));
    }

    let prepared = studio_assets::prepare_studio_assets(&dist_dir, &out_dir)
        .unwrap_or_else(|error| panic!("Studio asset verification failed: {error}"));
    for directory in prepared.watched_directories {
        emit_rerun_if_changed(&manifest_dir, &directory);
    }
    for file in prepared.watched_files {
        emit_rerun_if_changed(&manifest_dir, &file);
    }

    fs::write(out_dir.join("studio_assets.rs"), prepared.generated_source)
        .expect("write verified embedded Studio asset table");
}

fn emit_rerun_if_changed(manifest_dir: &Path, path: &Path) {
    let relative = studio_assets::safe_rerun_path(manifest_dir, path)
        .unwrap_or_else(|error| panic!("Invalid Cargo watch path: {error}"));
    println!("cargo:rerun-if-changed={relative}");
}
