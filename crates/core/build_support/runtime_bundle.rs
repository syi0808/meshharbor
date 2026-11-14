use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Component, Path, PathBuf},
};

const BACKENDS: [&str; 2] = ["cuda", "mlx"];
const RUNTIME_SUBDIRECTORIES: [&str; 2] = ["upstream", "managed"];

#[derive(Debug)]
pub struct PreparedBundle {
    pub generated_source: String,
    pub watched_files: Vec<PathBuf>,
    pub watched_directories: Vec<PathBuf>,
}

#[derive(Debug)]
struct AssetEntry {
    backend: String,
    path: String,
    sha256: String,
    size_bytes: u64,
}

impl AssetEntry {
    fn key(&self) -> String {
        format!("{}/{}", self.backend, self.path)
    }
}

/// Verifies a pinned manifest and all named source bytes, then stages only those
/// verified bytes into OUT_DIR. Any other distributable file under the runtime
/// source directories fails the build rather than being copied into the app.
pub fn prepare_bundle(
    workspace_root: &Path,
    manifest_path: &Path,
    out_dir: &Path,
    expected_manifest_sha256: Option<&str>,
    expected_file_count: Option<usize>,
) -> Result<PreparedBundle, String> {
    let manifest_metadata = fs::symlink_metadata(manifest_path)
        .map_err(|error| format!("Could not inspect runtime bundle manifest: {error}"))?;
    if !manifest_metadata.file_type().is_file() {
        return Err("Runtime bundle manifest must be a regular file.".into());
    }

    let manifest_bytes = fs::read(manifest_path)
        .map_err(|error| format!("Could not read runtime bundle manifest: {error}"))?;
    if let Some(expected) = expected_manifest_sha256 {
        let actual = sha256_hex(&manifest_bytes);
        if actual != expected {
            return Err(format!(
                "Runtime bundle manifest SHA-256 mismatch: expected {expected}, found {actual}."
            ));
        }
    }

    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("Runtime bundle manifest is invalid JSON: {error}"))?;
    if manifest
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err("Unsupported runtime bundle manifest schema.".into());
    }
    let raw_entries = manifest
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Runtime bundle manifest has no files array.".to_owned())?;

    let mut assets = Vec::with_capacity(raw_entries.len());
    let mut allowed = HashSet::with_capacity(raw_entries.len());
    for raw in raw_entries {
        let backend = raw
            .get("backend")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Runtime bundle entry has no backend.".to_owned())?;
        if !BACKENDS.contains(&backend) {
            return Err(format!("Unsupported runtime bundle backend: {backend}."));
        }

        let path = raw
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Runtime bundle entry has no path.".to_owned())?;
        validate_asset_path(path)?;

        let sha256 = raw
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("Runtime bundle entry {backend}/{path} has no SHA-256."))?;
        if !is_lower_hex_sha256(sha256) {
            return Err(format!(
                "Runtime bundle entry {backend}/{path} has an invalid SHA-256."
            ));
        }

        let size_bytes = raw
            .get("size_bytes")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("Runtime bundle entry {backend}/{path} has no size."))?;

        let asset = AssetEntry {
            backend: backend.to_owned(),
            path: path.to_owned(),
            sha256: sha256.to_owned(),
            size_bytes,
        };
        if !allowed.insert(asset.key()) {
            return Err(format!(
                "Duplicate runtime bundle manifest entry: {backend}/{path}."
            ));
        }
        assets.push(asset);
    }
    if assets.is_empty() {
        return Err("Runtime bundle manifest must contain at least one file.".into());
    }
    if let Some(expected) = expected_file_count
        && assets.len() != expected
    {
        return Err(format!(
            "Runtime bundle manifest contains {} files; expected {expected}.",
            assets.len()
        ));
    }
    assets.sort_by(|left, right| {
        left.backend
            .cmp(&right.backend)
            .then_with(|| left.path.cmp(&right.path))
    });

    let mut watched_files = BTreeSet::new();
    let mut watched_directories = BTreeSet::new();
    let mut unlisted = BTreeSet::new();
    for backend in BACKENDS {
        let backend_root = workspace_root.join("runtimes/pixal3d").join(backend);
        for subdirectory in RUNTIME_SUBDIRECTORIES {
            scan_directory(
                &backend_root,
                backend,
                &backend_root.join(subdirectory),
                &allowed,
                &mut watched_files,
                &mut watched_directories,
                &mut unlisted,
            )?;
        }
    }
    if !unlisted.is_empty() {
        return Err(format!(
            "Unlisted runtime files are present; refusing to embed them. Remove them or add reviewed manifest entries: {}",
            unlisted.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    // Read and verify the complete allowlist before staging any bytes. The staged
    // copies, not mutable checkout paths, are what rustc later embeds.
    let mut verified_assets = Vec::with_capacity(assets.len());
    for asset in assets {
        let source_path = workspace_root
            .join("runtimes/pixal3d")
            .join(&asset.backend)
            .join(&asset.path);
        let metadata = fs::symlink_metadata(&source_path).map_err(|error| {
            format!(
                "Could not inspect runtime asset {}: {error}",
                source_path.display()
            )
        })?;
        if !metadata.file_type().is_file() {
            return Err(format!(
                "Runtime asset is not a regular file: {}.",
                source_path.display()
            ));
        }

        let bytes = fs::read(&source_path).map_err(|error| {
            format!(
                "Could not read runtime asset {}: {error}",
                source_path.display()
            )
        })?;
        if bytes.len() as u64 != asset.size_bytes {
            return Err(format!(
                "Runtime asset size mismatch for {}/{}: expected {}, found {}.",
                asset.backend,
                asset.path,
                asset.size_bytes,
                bytes.len()
            ));
        }
        let actual_sha256 = sha256_hex(&bytes);
        if actual_sha256 != asset.sha256 {
            return Err(format!(
                "Runtime asset SHA-256 mismatch for {}/{}: expected {}, found {}.",
                asset.backend, asset.path, asset.sha256, actual_sha256
            ));
        }
        verified_assets.push((asset, bytes));
    }

    let mut generated_source = String::new();
    for backend in ["mlx", "cuda"] {
        let symbol = match backend {
            "mlx" => "PIXAL3D_MLX_FILES",
            "cuda" => "PIXAL3D_CUDA_FILES",
            _ => unreachable!(),
        };
        generated_source.push_str(&format!(
            "pub(super) static {symbol}: &[(&str, &[u8])] = &[\n"
        ));
        for (asset, bytes) in verified_assets
            .iter()
            .filter(|(asset, _)| asset.backend == backend)
        {
            let staged_path = out_dir
                .join("runtime-assets")
                .join(&asset.backend)
                .join(&asset.path);
            let parent = staged_path.parent().ok_or_else(|| {
                format!(
                    "Runtime asset has no staging directory: {}.",
                    staged_path.display()
                )
            })?;
            fs::create_dir_all(parent).map_err(|error| {
                format!("Could not create runtime asset staging directory: {error}")
            })?;
            fs::write(&staged_path, bytes)
                .map_err(|error| format!("Could not stage verified runtime asset: {error}"))?;

            let embedded_path = format!("/runtime-assets/{backend}/{}", asset.path);
            generated_source.push_str(&format!(
                "    ({:?}, include_bytes!(concat!(env!(\"OUT_DIR\"), {:?}))),\n",
                asset.path, embedded_path
            ));
        }
        generated_source.push_str("];\n");
    }

    watched_files.insert(manifest_path.to_path_buf());
    Ok(PreparedBundle {
        generated_source,
        watched_files: watched_files.into_iter().collect(),
        watched_directories: watched_directories.into_iter().collect(),
    })
}

fn validate_asset_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.contains('\\') || path.starts_with('/') {
        return Err(format!("Invalid runtime bundle path: {path:?}."));
    }
    let mut components = path.split('/');
    let first = components
        .next()
        .ok_or_else(|| format!("Invalid runtime bundle path: {path:?}."))?;
    if !RUNTIME_SUBDIRECTORIES.contains(&first) {
        return Err(format!(
            "Runtime bundle path must be under upstream/ or managed/: {path:?}."
        ));
    }
    let rest: Vec<_> = components.collect();
    if rest.is_empty()
        || rest.iter().any(|component| {
            component.is_empty()
                || *component == "."
                || *component == ".."
                || component.ends_with('.')
                || component.ends_with(' ')
                || component.chars().any(|character| {
                    character.is_control()
                        || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
                })
                || is_reserved_windows_device_name(component)
        })
    {
        return Err(format!("Invalid runtime bundle path: {path:?}."));
    }
    Ok(())
}

fn scan_directory(
    backend_root: &Path,
    backend: &str,
    directory: &Path,
    allowed: &HashSet<String>,
    watched_files: &mut BTreeSet<PathBuf>,
    watched_directories: &mut BTreeSet<PathBuf>,
    unlisted: &mut BTreeSet<String>,
) -> Result<(), String> {
    let relative_directory = runtime_relative_path(backend_root, directory)?;
    if relative_directory != "upstream" && relative_directory != "managed" {
        validate_asset_path(&relative_directory)?;
    }

    let metadata = fs::symlink_metadata(directory).map_err(|error| {
        format!(
            "Could not inspect runtime directory {}: {error}",
            directory.display()
        )
    })?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "Runtime source path is not a regular directory: {}.",
            directory.display()
        ));
    }
    watched_directories.insert(directory.to_path_buf());

    let mut entries: Vec<_> = fs::read_dir(directory)
        .map_err(|error| {
            format!(
                "Could not read runtime directory {}: {error}",
                directory.display()
            )
        })?
        .collect::<Result<_, _>>()
        .map_err(|error| format!("Could not read runtime directory entry: {error}"))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name();
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "Could not inspect runtime asset {}: {error}",
                path.display()
            )
        })?;
        if file_type.is_dir() {
            if file_name.to_str() == Some("__pycache__") {
                continue;
            }
            scan_directory(
                backend_root,
                backend,
                &path,
                allowed,
                watched_files,
                watched_directories,
                unlisted,
            )?;
        } else if file_type.is_file() {
            if path.extension().and_then(|extension| extension.to_str()) == Some("pyc") {
                continue;
            }
            let relative = runtime_relative_path(backend_root, &path)?;
            validate_asset_path(&relative)?;
            let key = format!("{backend}/{relative}");
            if !allowed.contains(&key) {
                unlisted.insert(key);
            }
            watched_files.insert(path);
        } else {
            return Err(format!(
                "Unsupported symlink or special runtime asset: {}.",
                path.display()
            ));
        }
    }
    Ok(())
}

fn runtime_relative_path(backend_root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(backend_root)
        .map_err(|error| format!("Runtime asset escaped its backend root: {error}"))?;
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => components.push(
                name.to_str()
                    .ok_or_else(|| format!("Runtime asset path is not UTF-8: {path:?}"))?,
            ),
            _ => return Err("Runtime asset path must be relative.".into()),
        }
    }
    Ok(components.join("/"))
}

fn is_reserved_windows_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{prepare_bundle, sha256_hex};
    use serde_json::json;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        manifest: PathBuf,
        output: PathBuf,
    }

    impl Fixture {
        fn new(contents: &[u8]) -> Self {
            let unique = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "meshharbor-runtime-bundle-{}-{unique}",
                std::process::id()
            ));
            if root.exists() {
                fs::remove_dir_all(&root).unwrap();
            }
            let output = root.join("target/out");
            let manifest = root.join("runtime_bundle_manifest.json");
            for backend in ["cuda", "mlx"] {
                for subdirectory in ["upstream", "managed"] {
                    fs::create_dir_all(
                        root.join("runtimes/pixal3d")
                            .join(backend)
                            .join(subdirectory),
                    )
                    .unwrap();
                }
            }
            let fixture = Self {
                root,
                manifest,
                output,
            };
            fixture.write_asset(contents);
            fixture.write_manifest("upstream/pixalcuda/cli.py", contents);
            fixture
        }

        fn asset_path(&self) -> PathBuf {
            self.root
                .join("runtimes/pixal3d/cuda/upstream/pixalcuda/cli.py")
        }

        fn write_asset(&self, contents: &[u8]) {
            let path = self.asset_path();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        fn write_manifest(&self, path: &str, contents: &[u8]) {
            let value = json!({
                "schema_version": 1,
                "files": [{
                    "backend": "cuda",
                    "path": path,
                    "sha256": sha256_hex(contents),
                    "size_bytes": contents.len()
                }]
            });
            fs::write(&self.manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        }

        fn prepare(&self) -> Result<super::PreparedBundle, String> {
            prepare_bundle(&self.root, &self.manifest, &self.output, None, None)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn unlisted_private_asset_fails_before_any_bytes_are_staged() {
        let fixture = Fixture::new(b"pinned");
        fs::write(
            fixture.root.join("runtimes/pixal3d/cuda/upstream/.env"),
            b"private-token",
        )
        .unwrap();

        let error = fixture.prepare().unwrap_err();
        assert!(error.contains("Unlisted runtime files"));
        assert!(error.contains("cuda/upstream/.env"));
        assert!(!error.contains("private-token"));
        assert!(
            !fixture
                .output
                .join("runtime-assets/cuda/upstream/pixalcuda/cli.py")
                .exists()
        );
    }

    #[test]
    fn modified_pinned_source_fails_hash_verification() {
        let fixture = Fixture::new(b"pinned");
        fixture.write_asset(b"changed");

        let error = fixture.prepare().unwrap_err();
        assert!(error.contains("SHA-256 mismatch"));
        assert!(
            !fixture
                .output
                .join("runtime-assets/cuda/upstream/pixalcuda/cli.py")
                .exists()
        );
    }

    #[test]
    fn staged_bytes_are_the_verified_bytes_even_if_source_changes_later() {
        let fixture = Fixture::new(b"pinned");

        let prepared = fixture.prepare().unwrap();
        fixture.write_asset(b"changed after verification");

        let staged = fixture
            .output
            .join("runtime-assets/cuda/upstream/pixalcuda/cli.py");
        assert_eq!(fs::read(staged).unwrap(), b"pinned");
        assert!(prepared.generated_source.contains(
            "include_bytes!(concat!(env!(\"OUT_DIR\"), \"/runtime-assets/cuda/upstream/pixalcuda/cli.py\"))"
        ));
    }

    #[test]
    fn traversing_and_non_portable_paths_are_rejected_before_source_access() {
        let fixture = Fixture::new(b"pinned");
        for path in [
            "../outside.txt",
            "upstream/../../outside.txt",
            "upstream/foo\\\\bar",
            "upstream/name?.py",
            "upstream/CON.txt",
            "upstream/trailing.",
            "managed/",
            "/upstream/outside.txt",
        ] {
            fixture.write_manifest(path, b"pinned");
            let error = fixture.prepare().unwrap_err();
            assert!(error.to_lowercase().contains("path"), "{path}: {error}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn backslash_filename_does_not_alias_an_allowlisted_path() {
        let fixture = Fixture::new(b"pinned");
        let unexpected = fixture
            .root
            .join("runtimes/pixal3d/cuda/upstream")
            .join("pixalcuda\\cli.py");
        fs::write(unexpected, b"unlisted").unwrap();

        let error = fixture.prepare().unwrap_err();
        assert!(error.contains("Invalid runtime bundle path"));
        assert!(
            !fixture
                .output
                .join("runtime-assets/cuda/upstream/pixalcuda/cli.py")
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn newline_directory_names_are_rejected_before_they_can_become_cargo_directives() {
        let fixture = Fixture::new(b"pinned");
        let injected = fixture
            .root
            .join("runtimes/pixal3d/cuda/upstream")
            .join("empty\ncargo:rustc-env=MESHHARBOR_INJECTED=yes");
        fs::create_dir_all(&injected).unwrap();

        let error = fixture.prepare().unwrap_err();
        assert!(error.to_lowercase().contains("path"));
        assert!(!error.contains('\n'));
        assert!(
            !fixture
                .output
                .join("runtime-assets/cuda/upstream/pixalcuda/cli.py")
                .exists()
        );
    }

    #[test]
    fn runtime_asset_paths_are_unique() {
        let fixture = Fixture::new(b"pinned");
        let value = json!({
            "schema_version": 1,
            "files": [
                {
                    "backend": "cuda",
                    "path": "upstream/pixalcuda/cli.py",
                    "sha256": sha256_hex(b"pinned"),
                    "size_bytes": 6
                },
                {
                    "backend": "cuda",
                    "path": "upstream/pixalcuda/cli.py",
                    "sha256": sha256_hex(b"pinned"),
                    "size_bytes": 6
                }
            ]
        });
        fs::write(
            &fixture.manifest,
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();

        let error = fixture.prepare().unwrap_err();
        assert!(error.contains("Duplicate runtime bundle manifest entry"));
    }

    #[test]
    fn expected_file_count_is_enforced() {
        let fixture = Fixture::new(b"pinned");
        let error = prepare_bundle(
            &fixture.root,
            &fixture.manifest,
            &fixture.output,
            None,
            Some(2),
        )
        .unwrap_err();

        assert!(error.contains("expected 2"));
        assert!(
            !fixture
                .output
                .join("runtime-assets/cuda/upstream/pixalcuda/cli.py")
                .exists()
        );
    }

    #[test]
    fn pinned_manifest_hash_is_enforced() {
        let fixture = Fixture::new(b"pinned");

        let wrong_hash = "0".repeat(64);
        let error = prepare_bundle(
            &fixture.root,
            &fixture.manifest,
            &fixture.output,
            Some(&wrong_hash),
            None,
        )
        .unwrap_err();
        assert!(error.contains("manifest SHA-256 mismatch"));
        assert!(
            !fixture
                .output
                .join("runtime-assets/cuda/upstream/pixalcuda/cli.py")
                .exists()
        );
    }

    #[test]
    fn generated_caches_are_deliberately_ignored() {
        let fixture = Fixture::new(b"pinned");
        let pycache = fixture
            .root
            .join("runtimes/pixal3d/mlx/upstream/__pycache__");
        fs::create_dir_all(&pycache).unwrap();
        fs::write(pycache.join("generated.pyc"), b"bytecode").unwrap();
        fs::write(
            fixture
                .root
                .join("runtimes/pixal3d/cuda/upstream/generated.pyc"),
            b"bytecode",
        )
        .unwrap();

        let prepared = fixture.prepare().unwrap();
        assert!(prepared.generated_source.contains("PIXAL3D_CUDA_FILES"));
        assert!(!prepared.generated_source.contains("generated.pyc"));
        assert!(prepared.watched_files.contains(&fixture.asset_path()));
        assert!(
            prepared
                .watched_directories
                .iter()
                .any(|directory| directory.ends_with("upstream"))
        );
    }
}
