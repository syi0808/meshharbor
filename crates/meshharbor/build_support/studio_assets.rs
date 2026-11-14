use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

const VITE_MANIFEST: &str = ".vite/manifest.json";
const STAGED_ROOT: &str = "meshharbor-studio-assets";

#[derive(Debug)]
pub struct PreparedStudioAssets {
    pub generated_source: String,
    pub watched_files: Vec<PathBuf>,
    pub watched_directories: Vec<PathBuf>,
}

pub fn safe_rerun_path(base: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(base)
        .map_err(|_| "Cargo watch path is outside the package root.".to_owned())?;
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or_else(|| "Cargo watch path must be valid UTF-8.".to_owned())?;
                if value.contains('\\') || value.chars().any(char::is_control) {
                    return Err("Cargo watch path contains a non-portable component.".into());
                }
                components.push(value.to_owned());
            }
            Component::ParentDir => components.push("..".to_owned()),
            Component::CurDir => {}
            _ => return Err("Cargo watch path must remain relative.".into()),
        }
    }
    if components.is_empty() {
        return Err("Cargo watch path must not be empty.".into());
    }
    Ok(components.join("/"))
}

pub fn prepare_studio_assets(
    dist_dir: &Path,
    out_dir: &Path,
) -> Result<PreparedStudioAssets, String> {
    let mut watched_directories = BTreeSet::from([dist_dir.to_path_buf()]);
    let mut watched_files = BTreeSet::new();
    let metadata = match fs::symlink_metadata(dist_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PreparedStudioAssets {
                generated_source: empty_source(),
                watched_files: Vec::new(),
                watched_directories: watched_directories.into_iter().collect(),
            });
        }
        Err(error) => return Err(format!("Could not inspect Studio dist directory: {error}")),
    };
    if !metadata.file_type().is_dir() {
        return Err("Studio dist path must be a regular directory, not a symlink or file.".into());
    }

    let manifest_path = dist_dir.join(VITE_MANIFEST);
    let manifest_bytes = read_regular_file(&manifest_path, "Vite asset manifest")?;
    watched_files.insert(manifest_path);
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("Vite asset manifest is invalid JSON: {error}"))?;
    let allowed_assets = manifest_asset_paths(&manifest)?;

    let mut actual_files = BTreeSet::new();
    let mut folded_paths = HashSet::new();
    scan_dist(
        dist_dir,
        dist_dir,
        &mut actual_files,
        &mut folded_paths,
        &mut watched_files,
        &mut watched_directories,
    )?;

    let mut expected_files = allowed_assets.clone();
    expected_files.insert(VITE_MANIFEST.to_owned());
    let unexpected: Vec<_> = actual_files.difference(&expected_files).cloned().collect();
    if !unexpected.is_empty() {
        return Err(format!(
            "Unlisted Studio build output files are present; refusing to embed them: {}",
            unexpected.join(", ")
        ));
    }
    let missing: Vec<_> = expected_files.difference(&actual_files).cloned().collect();
    if !missing.is_empty() {
        return Err(format!(
            "Vite asset manifest references missing Studio build output files: {}",
            missing.join(", ")
        ));
    }

    let stage_root = out_dir.join(STAGED_ROOT);
    ensure_directory(out_dir)?;
    ensure_directory(&stage_root)?;
    let mut generated_source = String::from(
        "pub struct StudioAsset { pub path: &'static str, pub content_type: &'static str, pub body: &'static [u8] }\npub static STUDIO_ASSETS: &[StudioAsset] = &[\n",
    );
    for relative in allowed_assets {
        let source = dist_dir.join(&relative);
        let bytes = read_regular_file(&source, &format!("Studio asset {relative}"))?;
        let expected_hash = sha256_hex(&bytes);
        let expected_size = bytes.len() as u64;
        stage_verified_asset(&stage_root, &relative, &bytes)?;
        let staged_path = stage_root.join(&relative);
        let staged_bytes =
            read_regular_file(&staged_path, &format!("staged Studio asset {relative}"))?;
        if staged_bytes.len() as u64 != expected_size || sha256_hex(&staged_bytes) != expected_hash
        {
            return Err(format!(
                "Staged Studio asset failed integrity verification: {relative}."
            ));
        }
        let embedded_path = format!("/{STAGED_ROOT}/{relative}");
        generated_source.push_str(&format!(
            "    // sha256={expected_hash}, size_bytes={expected_size}\n    StudioAsset {{ path: {:?}, content_type: {:?}, body: include_bytes!(concat!(env!(\"OUT_DIR\"), {:?})) }},\n",
            format!("/{relative}"),
            content_type(&relative),
            embedded_path
        ));
        watched_files.insert(source);
    }
    generated_source.push_str("];\n");

    Ok(PreparedStudioAssets {
        generated_source,
        watched_files: watched_files.into_iter().collect(),
        watched_directories: watched_directories.into_iter().collect(),
    })
}

fn empty_source() -> String {
    String::from(
        "pub struct StudioAsset { pub path: &'static str, pub content_type: &'static str, pub body: &'static [u8] }\npub static STUDIO_ASSETS: &[StudioAsset] = &[];\n",
    )
}

fn manifest_asset_paths(manifest: &serde_json::Value) -> Result<BTreeSet<String>, String> {
    let entries = manifest
        .as_object()
        .ok_or_else(|| "Vite asset manifest must be a JSON object.".to_owned())?;
    if !entries.contains_key("index.html") {
        return Err("Vite asset manifest does not contain the Studio index.html entry.".into());
    }
    let mut paths = BTreeSet::from(["index.html".to_owned()]);
    for (entry_name, entry) in entries {
        if !entry.is_object() {
            return Err(format!(
                "Vite manifest entry {entry_name:?} must be an object."
            ));
        }
        let file = entry
            .get("file")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("Vite manifest entry {entry_name:?} has no output file."))?;
        insert_manifest_path(&mut paths, file)?;
        for field in ["css", "assets"] {
            if let Some(values) = entry.get(field) {
                let values = values.as_array().ok_or_else(|| {
                    format!("Vite manifest entry {entry_name:?} field {field:?} must be an array.")
                })?;
                for value in values {
                    let path = value.as_str().ok_or_else(|| {
                        format!("Vite manifest entry {entry_name:?} field {field:?} contains a non-string path.")
                    })?;
                    insert_manifest_path(&mut paths, path)?;
                }
            }
        }
    }
    Ok(paths)
}

fn insert_manifest_path(paths: &mut BTreeSet<String>, path: &str) -> Result<(), String> {
    validate_asset_path(path)?;
    if path == VITE_MANIFEST {
        return Err("Vite asset manifest cannot list itself as a Studio asset.".into());
    }
    paths.insert(path.to_owned());
    Ok(())
}

fn validate_asset_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return Err(format!("Invalid Studio asset path: {path:?}."));
    }
    if path.split('/').any(|component| {
        component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with('.')
            || component.ends_with(' ')
            || component.chars().any(|character| {
                character.is_control()
                    || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
            })
            || is_reserved_windows_device_name(component)
    }) {
        return Err(format!("Invalid Studio asset path: {path:?}."));
    }
    Ok(())
}

fn is_reserved_windows_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
        })
}

fn scan_dist(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<String>,
    folded_paths: &mut HashSet<String>,
    watched_files: &mut BTreeSet<PathBuf>,
    watched_directories: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| format!("Could not inspect Studio output directory: {error}"))?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "Studio output directory is not a regular directory: {}.",
            directory.display()
        ));
    }
    watched_directories.insert(directory.to_path_buf());
    let mut entries: Vec<_> = fs::read_dir(directory)
        .map_err(|error| format!("Could not read Studio output directory: {error}"))?
        .collect::<Result<_, _>>()
        .map_err(|error| format!("Could not read Studio output entry: {error}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("Could not inspect Studio output entry: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Studio output contains a symlink; refusing to embed it: {}.",
                path.display()
            ));
        }
        if metadata.file_type().is_dir() {
            let relative = relative_path(root, &path)?;
            record_portable_path(&relative, folded_paths)?;
            scan_dist(
                root,
                &path,
                files,
                folded_paths,
                watched_files,
                watched_directories,
            )?;
        } else if metadata.file_type().is_file() {
            let relative = relative_path(root, &path)?;
            record_portable_path(&relative, folded_paths)?;
            files.insert(relative);
            watched_files.insert(path);
        } else {
            return Err(format!(
                "Studio output contains a special file; refusing to embed it: {}.",
                path.display()
            ));
        }
    }
    Ok(())
}

fn record_portable_path(path: &str, folded_paths: &mut HashSet<String>) -> Result<(), String> {
    validate_asset_path(path)?;
    if !folded_paths.insert(path.to_lowercase()) {
        return Err(format!(
            "Studio output contains a case-insensitive path collision: {path:?}."
        ));
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Studio output path escaped its dist directory.".to_owned())?;
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or_else(|| "Studio output paths must be valid UTF-8.".to_owned())?;
                if value.contains('\\') {
                    return Err(format!(
                        "Studio output path is not portable across operating systems: {value:?}."
                    ));
                }
                components.push(value.to_owned());
            }
            _ => return Err("Studio output path contains a non-normal component.".into()),
        }
    }
    let result = components.join("/");
    validate_asset_path(&result)?;
    Ok(result)
}

fn read_regular_file(path: &Path, description: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect {description}: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "{description} must be a regular file, not a symlink or special file."
        ));
    }
    fs::read(path).map_err(|error| format!("Could not read {description}: {error}"))
}

fn stage_verified_asset(stage_root: &Path, relative: &str, bytes: &[u8]) -> Result<(), String> {
    let relative_path = Path::new(relative);
    let parent = relative_path
        .parent()
        .ok_or_else(|| format!("Studio asset has no parent directory: {relative:?}."))?;
    let mut target_parent = stage_root.to_path_buf();
    for component in parent.components() {
        match component {
            Component::Normal(name) => target_parent.push(name),
            _ => return Err(format!("Invalid Studio staging path: {relative:?}.")),
        }
        ensure_directory(&target_parent)?;
    }
    let target = stage_root.join(relative_path);
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_file() => {
            fs::remove_file(&target)
                .map_err(|error| format!("Could not replace staged Studio asset: {error}"))?;
        }
        Ok(_) => {
            return Err(format!(
                "Staged Studio asset path is not a regular file: {}.",
                target.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Could not inspect staged Studio asset: {error}")),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|error| format!("Could not create staged Studio asset: {error}"))?;
    file.write_all(bytes)
        .map_err(|error| format!("Could not write staged Studio asset: {error}"))
}

fn ensure_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "Studio staging path must be a regular directory: {}.",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)
            .map_err(|error| format!("Could not create Studio staging directory: {error}")),
        Err(error) => Err(format!(
            "Could not inspect Studio staging directory: {error}"
        )),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn content_type(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
    {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "meshharbor-studio-assets-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        let dist = root.join("dist");
        let out = root.join("out");
        fs::create_dir_all(dist.join("assets")).unwrap();
        fs::create_dir_all(dist.join(".vite")).unwrap();
        fs::create_dir_all(&out).unwrap();
        fs::write(dist.join("index.html"), "<html></html>").unwrap();
        fs::write(dist.join("assets/app.js"), "console.log('ok')").unwrap();
        fs::write(dist.join("assets/app.css"), "body{}").unwrap();
        fs::write(
            dist.join(VITE_MANIFEST),
            serde_json::to_vec(&json!({
                "index.html": {
                    "file": "assets/app.js",
                    "css": ["assets/app.css"],
                    "isEntry": true
                }
            }))
            .unwrap(),
        )
        .unwrap();
        (root, dist, out)
    }

    #[test]
    fn cargo_watch_paths_do_not_include_a_controlled_checkout_prefix() {
        let package = PathBuf::from("checkout\nworkspace").join("crates/meshharbor");
        let watch = package.join("../../apps/studio/package.json");
        let relative = safe_rerun_path(&package, &watch).unwrap();
        assert_eq!(relative, "../../apps/studio/package.json");
        assert!(!relative.chars().any(char::is_control));
    }

    #[test]
    fn manifest_assets_are_staged_and_unlisted_files_are_rejected() {
        let (root, dist, out) = fixture("manifest");
        let prepared = prepare_studio_assets(&dist, &out).unwrap();
        assert!(prepared.generated_source.contains("path: \"/index.html\""));
        assert!(
            prepared
                .generated_source
                .contains("path: \"/assets/app.js\"")
        );
        assert!(
            prepared
                .generated_source
                .contains("path: \"/assets/app.css\"")
        );
        assert!(!prepared.generated_source.contains(".vite/manifest.json"));
        assert!(prepared.watched_files.contains(&dist.join("assets/app.js")));
        assert!(prepared.watched_directories.contains(&dist));
        assert_eq!(
            fs::read(out.join(STAGED_ROOT).join("assets/app.js")).unwrap(),
            b"console.log('ok')"
        );
        fs::write(dist.join("assets/app.js"), "changed bytes").unwrap();
        assert_eq!(
            fs::read(out.join(STAGED_ROOT).join("assets/app.js")).unwrap(),
            b"console.log('ok')"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unlisted_private_file_fails_before_staging_any_asset() {
        let (root, dist, out) = fixture("unlisted");
        fs::write(dist.join(".env"), "local secret").unwrap();
        let error = prepare_studio_assets(&dist, &out).unwrap_err();
        assert!(error.contains("Unlisted Studio build output"));
        assert!(!out.join(STAGED_ROOT).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_in_studio_output_fails_closed() {
        use std::os::unix::fs::symlink;
        let (root, dist, out) = fixture("symlink");
        let outside = root.join("outside-secret");
        fs::write(&outside, "private").unwrap();
        symlink(&outside, dist.join("assets/linked.js")).unwrap();
        let error = prepare_studio_assets(&dist, &out).unwrap_err();
        assert!(error.contains("symlink"));
        assert!(!out.join(STAGED_ROOT).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_manifest_path_fails_before_staging() {
        let (root, dist, out) = fixture("invalid");
        fs::write(
            dist.join(VITE_MANIFEST),
            serde_json::to_vec(&json!({
                "index.html": {
                    "file": "../outside.js",
                    "isEntry": true
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let error = prepare_studio_assets(&dist, &out).unwrap_err();
        assert!(error.contains("Invalid Studio asset path"));
        assert!(!out.join(STAGED_ROOT).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_manifest_fails_if_dist_directory_exists() {
        let root = std::env::temp_dir().join(format!(
            "meshharbor-studio-assets-no-manifest-{}",
            uuid::Uuid::new_v4()
        ));
        let dist = root.join("dist");
        let out = root.join("out");
        fs::create_dir_all(&dist).unwrap();
        fs::create_dir_all(&out).unwrap();
        let error = prepare_studio_assets(&dist, &out).unwrap_err();
        assert!(error.contains("Vite asset manifest"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn absent_dist_preserves_backend_only_build() {
        let root = std::env::temp_dir().join(format!(
            "meshharbor-studio-assets-absent-{}",
            uuid::Uuid::new_v4()
        ));
        let dist = root.join("missing");
        let out = root.join("out");
        fs::create_dir_all(&out).unwrap();
        let prepared = prepare_studio_assets(&dist, &out).unwrap();
        assert!(
            prepared
                .generated_source
                .contains("STUDIO_ASSETS: &[StudioAsset] = &[]")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
