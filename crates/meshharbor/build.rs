use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let studio_dir = manifest_dir.join("../../apps/studio");
    let dist_dir = manifest_dir.join("../../apps/studio/dist");
    println!("cargo:rerun-if-changed={}", studio_dir.display());
    println!("cargo:rerun-if-changed={}", dist_dir.display());
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let generated = out_dir.join("studio_assets.rs");
    let mut files = Vec::new();
    if dist_dir.is_dir() {
        collect_files(&dist_dir, &mut files).expect("enumerate Studio build output");
    }
    files.sort();
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }

    let mut source = String::from(
        "pub struct StudioAsset { pub path: &'static str, pub content_type: &'static str, pub body: &'static [u8] }\n",
    );
    source.push_str("pub static STUDIO_ASSETS: &[StudioAsset] = &[\n");
    for file in files {
        let relative = file
            .strip_prefix(&dist_dir)
            .expect("Studio asset is under dist")
            .to_string_lossy()
            .replace('\\', "/");
        let path = format!("/{relative}");
        let absolute = file.canonicalize().expect("canonicalize Studio asset");
        let absolute = absolute.to_str().expect("Studio asset path must be UTF-8");
        let content_type = content_type(&file);
        source.push_str(&format!(
            "StudioAsset {{ path: {path:?}, content_type: {content_type:?}, body: include_bytes!({absolute:?}) }},\n"
        ));
    }
    source.push_str("];\n");
    fs::write(generated, source).expect("write embedded Studio asset table");
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_files(&entry.path(), files)?;
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") | Some("map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}
