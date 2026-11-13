use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_root = manifest_dir.join("../..");
    let mlx_root = workspace_root.join("runtimes/pixal3d/mlx");
    let cuda_root = workspace_root.join("runtimes/pixal3d/cuda");
    let mut generated = String::new();
    for (backend, root, symbol) in [
        ("mlx", mlx_root, "PIXAL3D_MLX_FILES"),
        ("cuda", cuda_root, "PIXAL3D_CUDA_FILES"),
    ] {
        let mut files = Vec::new();
        for directory in [root.join("upstream"), root.join("managed")] {
            collect_files(&directory, &mut files);
        }
        files.sort();
        generated.push_str(&format!("pub(super) static {symbol}: &[(&str, &[u8])] = &[\n"));
        for file in files {
            let relative = file
                .strip_prefix(&root)
                .expect("runtime files are under the backend root")
                .to_str()
                .expect("runtime asset paths must be UTF-8")
                .replace('\\', "/");
            println!("cargo:rerun-if-changed={}", file.display());
            generated.push_str(&format!(
                "    ({relative:?}, include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../runtimes/pixal3d/{backend}/{relative}\"))),\n"
            ));
        }
        generated.push_str("];\n");
        println!("cargo:rerun-if-changed={}", root.display());
    }

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"))
        .join("pixal3d_runtime_bundle.rs");
    fs::write(output, generated).expect("write embedded runtime manifest");
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", directory.display()));
    for entry in entries {
        let entry = entry.expect("read runtime directory entry");
        let path = entry.path();
        let file_type = entry.file_type().expect("inspect runtime asset type");
        if file_type.is_dir() && entry.file_name().to_str() == Some("__pycache__") {
            continue;
        }
        if file_type.is_dir() {
            collect_files(&path, files);
        } else if file_type.is_file()
            && path.extension().and_then(|extension| extension.to_str()) != Some("pyc")
        {
            files.push(path);
        } else {
            panic!("unsupported symlink or special runtime asset: {}", path.display());
        }
    }
}
