use std::{
    env,
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use serde::Serialize;

use crate::{
    CoreError, CoreResult,
    huggingface_browser::installed_browser_cli,
    job_execution::JobExecution,
    model_storage::{is_real_directory, symlink_file, weight_matches_sha},
};

pub(crate) const DINO_REPO: &str = "facebook/dinov3-vitl16-pretrain-lvd1689m";
pub(crate) const NAF_SHA256: &str =
    "c096c1ab2217a5c3ac136365f721685e2201379cb69d509cfb0261183847c98f";
pub(crate) const PIXAL_REPO: &str = "TencentARC/Pixal3D";
pub(crate) const PIXAL_REVISION: &str = "0b31f9160aa400719af409098bff7936a932f726";
pub(crate) const PIXAL_MULTIVIEW_REVISION: &str = "b0cb2e1b794cab9aa0ac38a95d794a4d9337437f";
const PIXAL_MULTIVIEW_FILES: &[&str] = &[
    "pipeline_mv.json",
    "ckpts/ss_flow_img_dit_1_3B_64_bf16_mv.safetensors",
    "ckpts/ss_dec_conv3d_16l8_fp16.safetensors",
    "ckpts/slat_flow_img2shape_dit_1_3B_512_bf16_mv.safetensors",
    "ckpts/shape_dec_next_dc_f16c32_fp16.safetensors",
    "ckpts/slat_flow_img2shape_dit_1_3B_1024_bf16_mv.safetensors",
    "ckpts/slat_flow_imgshape2tex_dit_1_3B_1024_bf16_mv.safetensors",
    "ckpts/tex_dec_next_dc_f16c32_fp16.safetensors",
];
pub(crate) const PIXAL_REPOSITORY: &str = "models--TencentARC--Pixal3D";
pub(crate) const PIXAL_CHECKPOINTS: &[&str] = &[
    "ss_flow_img_dit_1_3B_64_bf16.safetensors",
    "ss_dec_conv3d_16l8_fp16.safetensors",
    "slat_flow_img2shape_dit_1_3B_512_bf16.safetensors",
    "shape_dec_next_dc_f16c32_fp16.safetensors",
    "slat_flow_img2shape_dit_1_3B_1024_bf16.safetensors",
    "slat_flow_imgshape2tex_dit_1_3B_1024_bf16.safetensors",
    "tex_dec_next_dc_f16c32_fp16.safetensors",
];

pub(crate) fn shared_hub_cache() -> Option<PathBuf> {
    let configured_path = |key| {
        let path = PathBuf::from(env::var_os(key)?);
        if let Ok(relative) = path.strip_prefix("~") {
            Some(PathBuf::from(env::var_os("HOME")?).join(relative))
        } else {
            path.is_absolute().then_some(path)
        }
    };
    configured_path("HF_HUB_CACHE")
        .or_else(|| configured_path("HUGGINGFACE_HUB_CACHE"))
        .or_else(|| configured_path("HF_HOME").map(|home| home.join("hub")))
        .or_else(|| {
            let cache = configured_path("XDG_CACHE_HOME")
                .or_else(|| configured_path("HOME").map(|home| home.join(".cache")))?;
            Some(cache.join("huggingface/hub"))
        })
}

fn pixal_snapshot(hub: &Path, revision: &str) -> PathBuf {
    hub_snapshot(hub, PIXAL_REPOSITORY, revision)
}

fn hub_snapshot(hub: &Path, repo_directory: &str, revision: &str) -> PathBuf {
    hub.join(repo_directory).join("snapshots").join(revision)
}

fn repo_directory(repo: &str) -> String {
    format!("models--{}", repo.replace('/', "--"))
}

fn present(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn shared_file(hub: &Path, repo: &str, revision: &str, relative: &str) -> Option<PathBuf> {
    let snapshot = hub_snapshot(hub, &repo_directory(repo), revision);
    let source = snapshot.join(relative).canonicalize().ok()?;
    (present(&source) && snapshot_owns_file(&snapshot, &source)).then_some(source)
}

fn snapshot_owns_file(snapshot: &Path, source: &Path) -> bool {
    source.starts_with(snapshot)
        || snapshot
            .parent()
            .and_then(Path::parent)
            .is_some_and(|repo| {
                source.parent() == Some(repo.join("blobs").as_path())
                    && source
                        .file_name()
                        .is_some_and(|name| !name.to_string_lossy().ends_with(".incomplete"))
            })
}

#[derive(Serialize)]
pub(crate) struct PixalCacheStatus {
    local_checkpoints: usize,
    reusable_checkpoints: usize,
    total_checkpoints: usize,
}

pub(crate) fn pixal_cache_status(home: &Path, shared_cache: Option<&Path>) -> PixalCacheStatus {
    let local = pixal_snapshot(&home.join(".cache/huggingface/hub"), PIXAL_REVISION);
    let shared = shared_cache.and_then(|path| path.canonicalize().ok());
    let mut status = PixalCacheStatus {
        local_checkpoints: 0,
        reusable_checkpoints: 0,
        total_checkpoints: PIXAL_CHECKPOINTS.len(),
    };
    for filename in PIXAL_CHECKPOINTS {
        let relative = format!("ckpts/{filename}");
        if present(&local.join(&relative)) {
            status.local_checkpoints += 1;
        } else if shared
            .as_deref()
            .and_then(|hub| shared_file(hub, PIXAL_REPO, PIXAL_REVISION, &relative))
            .is_some()
        {
            status.reusable_checkpoints += 1;
        }
    }
    status
}

pub(crate) fn pixal_files(include_configs: bool) -> Vec<String> {
    let mut files = vec!["pipeline.json".into()];
    for filename in PIXAL_CHECKPOINTS {
        files.push(format!("ckpts/{filename}"));
        if include_configs {
            files.push(format!(
                "ckpts/{}",
                filename.replace(".safetensors", ".json")
            ));
        }
    }
    files
}

pub(crate) fn download_pixal3d(
    runtime_root: &Path,
    home: &Path,
    shared_cache: Option<&Path>,
    include_configs: bool,
    execution: crate::job_execution::JobExecution<'_>,
) -> CoreResult<()> {
    download_snapshot(
        runtime_root,
        home,
        shared_cache,
        (PIXAL_REPO, PIXAL_REVISION),
        pixal_files(include_configs),
        "2",
        execution,
    )
}

pub(crate) fn multiview_files_present(home: &Path) -> bool {
    let local = pixal_snapshot(
        &home.join(".cache/huggingface/hub"),
        PIXAL_MULTIVIEW_REVISION,
    );
    PIXAL_MULTIVIEW_FILES
        .iter()
        .all(|file| present(&local.join(file)))
}

pub(crate) fn download_pixal3d_multiview(
    runtime_root: &Path,
    home: &Path,
    execution: crate::job_execution::JobExecution<'_>,
) -> CoreResult<()> {
    download_snapshot(
        runtime_root,
        home,
        shared_hub_cache().as_deref(),
        (PIXAL_REPO, PIXAL_MULTIVIEW_REVISION),
        PIXAL_MULTIVIEW_FILES
            .iter()
            .map(ToString::to_string)
            .collect(),
        "2",
        execution,
    )
}

pub(crate) fn download_snapshot(
    runtime_root: &Path,
    home: &Path,
    shared_cache: Option<&Path>,
    snapshot: (&str, &str),
    files: Vec<String>,
    workers: &str,
    execution: JobExecution<'_>,
) -> CoreResult<()> {
    let (repo, revision) = snapshot;
    let local = hub_snapshot(
        &home.join(".cache/huggingface/hub"),
        &repo_directory(repo),
        revision,
    );
    let missing = link_snapshot_files(
        home,
        &local,
        shared_cache,
        snapshot,
        &files,
        true,
        execution,
    )?;
    if missing.is_empty() {
        return Ok(());
    }
    download(
        runtime_root,
        home,
        repo,
        revision,
        missing.into_iter().chain(["--max-workers", workers]),
        execution,
    )
}

pub(crate) fn link_snapshot_files<'a>(
    scope: &Path,
    destination: &Path,
    shared_cache: Option<&Path>,
    snapshot: (&str, &str),
    files: &'a [String],
    keep_local: bool,
    execution: JobExecution<'_>,
) -> CoreResult<Vec<&'a str>> {
    let (repo, revision) = snapshot;
    let shared = shared_cache.and_then(|path| path.canonicalize().ok());
    let destination = private_path(scope, destination, shared.as_deref())?;
    let scope = scope.canonicalize()?;
    let mut missing = Vec::new();
    let mut linked = 0;
    for relative in files {
        execution.checkpoint()?;
        if !normal_path(Path::new(relative)) {
            return Err(CoreError::InvalidInput(
                "Invalid pinned model file path.".into(),
            ));
        }
        let target = destination.join(relative);
        prepare_private_parent(&scope, &target)?;
        let source = shared
            .as_deref()
            .and_then(|hub| shared_file(hub, repo, revision, relative));
        match target.symlink_metadata() {
            Ok(metadata) if metadata.is_symlink() => {
                if let Ok(resolved) = target.canonicalize()
                    && present(&resolved)
                    && (source.as_ref() == Some(&resolved)
                        || (keep_local && snapshot_owns_file(&destination, &resolved)))
                {
                    continue;
                }
                fs::remove_file(&target)?;
            }
            Ok(metadata) if metadata.is_file() => {
                if keep_local {
                    if metadata.len() > 0 {
                        continue;
                    }
                    fs::remove_file(&target)?;
                } else {
                    missing.push(relative.as_str());
                    continue;
                }
            }
            Ok(_) => return Err(not_private_file(&target)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if source.is_some_and(|source| symlink_file(&source, &target).is_ok()) {
            linked += 1;
        } else {
            missing.push(relative.as_str());
        }
    }
    execution.note(&format!(
        "{repo}: linked {linked} pinned files from the shared cache; {} files need download or verification.",
        missing.len()
    ))?;
    Ok(missing)
}

fn normal_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn not_private_file(path: &Path) -> CoreError {
    CoreError::OperationUnavailable(format!(
        "Model files must stay in private storage: {}",
        path.display()
    ))
}

fn private_path(scope: &Path, target: &Path, shared: Option<&Path>) -> CoreResult<PathBuf> {
    let relative = target
        .strip_prefix(scope)
        .map_err(|_| not_private_file(target))?;
    if !normal_path(relative) || !is_real_directory(&scope.symlink_metadata()?) {
        return Err(not_private_file(target));
    }
    let path = scope.canonicalize()?.join(relative);
    if shared.is_some_and(|hub| path.starts_with(hub)) {
        return Err(not_private_file(&path));
    }
    Ok(path)
}

fn prepare_private_parent(scope: &Path, target: &Path) -> CoreResult<()> {
    let parent = target.parent().expect("pinned file has a parent");
    let relative = parent.strip_prefix(scope).expect("checked private path");
    let mut directory = scope.to_path_buf();
    for component in relative.components() {
        directory.push(component);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if !is_real_directory(&metadata) => {
                return Err(not_private_file(&directory));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&directory)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn link_cached_naf(
    scope: &Path,
    file: &Path,
    shared_cache: Option<&Path>,
    execution: JobExecution<'_>,
) -> CoreResult<()> {
    let shared = shared_cache.and_then(|hub| hub.canonicalize().ok());
    let target = private_path(scope, file, shared.as_deref())?;
    prepare_private_parent(&scope.canonicalize()?, &target)?;
    let temporary = target.with_extension("download");
    match temporary.symlink_metadata() {
        Ok(metadata) if !metadata.is_file() => return Err(not_private_file(&temporary)),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if target
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_file())
    {
        return Ok(());
    }
    let cached = shared.and_then(|hub| {
        let directory = hub.join("pixal3d-cuda");
        let source = directory.join("naf_release.pth").canonicalize().ok()?;
        (present(&source) && source.starts_with(directory)).then_some(source)
    });
    let cached = match cached {
        Some(path) => match weight_matches_sha(&path, NAF_SHA256, execution) {
            Ok(true) => Some(path),
            Ok(false) | Err(CoreError::Io(_)) => None,
            Err(error) => return Err(error),
        },
        None => None,
    };
    match target.symlink_metadata() {
        Ok(metadata) if metadata.is_symlink() => {
            if cached.as_ref() == target.canonicalize().ok().as_ref() && cached.is_some() {
                return Ok(());
            }
            fs::remove_file(&target)?;
        }
        Ok(_) => return Err(not_private_file(&target)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if cached.is_some_and(|source| symlink_file(&source, &target).is_ok()) {
        execution.note("Linked the SHA-256-verified NAF checkpoint from the shared cache.")?;
    }
    Ok(())
}

fn download(
    runtime_root: &Path,
    home: &Path,
    repo: &str,
    revision: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    execution: crate::job_execution::JobExecution<'_>,
) -> CoreResult<()> {
    let executable =
        installed_browser_cli(runtime_root).unwrap_or_else(|| runtime_root.join(".venv/bin/hf"));
    if !executable.is_file() {
        return Err(CoreError::OperationUnavailable(
            "The managed Hugging Face CLI is missing. Set up the backend again or use browser sign-in to prepare it."
                .into(),
        ));
    }
    let mut command = Command::new(executable);
    command
        .env_clear()
        .env("HOME", home)
        .env("HF_HOME", home.join(".cache/huggingface"))
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("HF_HUB_DISABLE_UPDATE_CHECK", "1")
        .env("PYTHONNOUSERSITE", "1")
        .args(["download", repo, "--revision", revision])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::io::stderr()))
        .stderr(Stdio::inherit());
    let status = execution.status(&mut command).map_err(|error| {
        CoreError::OperationUnavailable(format!("Could not start hf download for {repo}: {error}"))
    })?;
    if !status.success() {
        return Err(CoreError::OperationUnavailable(format!(
            "Hugging Face download failed for {repo} ({status}). Check your connection and model access, then retry to reuse the local HF cache."
        )));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use uuid::Uuid;

    #[test]
    fn complete_pinned_shared_snapshot_is_reused_without_running_hf() {
        let root = env::temp_dir().join(format!("meshharbor-cache-reuse-{}", Uuid::new_v4()));
        let home = root.join("runtime/home");
        let shared = root.join("shared");
        let snapshot = pixal_snapshot(&shared, PIXAL_REVISION);
        let blob = shared.join(PIXAL_REPOSITORY).join("blobs/test-blob");
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::create_dir_all(snapshot.join("ckpts")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(&blob, b"cached fixture").unwrap();
        for file in pixal_files(true) {
            symlink(&blob, snapshot.join(file)).unwrap();
        }

        let status = pixal_cache_status(&home, Some(&shared));
        assert_eq!(status.local_checkpoints, 0);
        assert_eq!(status.reusable_checkpoints, PIXAL_CHECKPOINTS.len());
        download_pixal3d(
            &root.join("without-hf"),
            &home,
            Some(&shared),
            true,
            Default::default(),
        )
        .unwrap();
        let status = pixal_cache_status(&home, Some(&shared));
        assert_eq!(status.local_checkpoints, PIXAL_CHECKPOINTS.len());
        assert_eq!(status.reusable_checkpoints, 0);
        let linked = pixal_snapshot(&home.join(".cache/huggingface/hub"), PIXAL_REVISION)
            .join("pipeline.json");
        assert!(linked.symlink_metadata().unwrap().is_symlink());
        assert_eq!(fs::read_link(linked).unwrap(), blob.canonicalize().unwrap());
        fs::remove_dir_all(home.join(".cache")).unwrap();
        assert_eq!(
            fs::read(snapshot.join("pipeline.json")).unwrap(),
            b"cached fixture"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shared_cache_links_pinned_external_weights_and_repairs_dangling_links() {
        let root = env::temp_dir().join(format!("meshharbor-cache-external-{}", Uuid::new_v4()));
        let runtime = root.join("runtime");
        let weights = runtime.join("weights/model");
        let hub = root.join("shared");
        let repo = "microsoft/TRELLIS-image-large";
        let revision = "reviewed";
        let snapshot = hub_snapshot(&hub, &repo_directory(repo), revision);
        let cached = snapshot.join("model.safetensors");
        let other = hub_snapshot(&hub, &repo_directory(repo), "different").join("config.json");
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&snapshot).unwrap();
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&cached, b"pinned fixture").unwrap();
        fs::write(&other, b"different revision").unwrap();
        symlink(&other, snapshot.join("config.json")).unwrap();
        let files = vec!["model.safetensors".into(), "config.json".into()];
        let prepare = || {
            link_snapshot_files(
                &runtime,
                &weights,
                Some(&hub),
                (repo, revision),
                &files,
                false,
                Default::default(),
            )
        };
        assert_eq!(prepare().unwrap(), ["config.json"]);
        assert_eq!(
            fs::read_link(weights.join("model.safetensors")).unwrap(),
            cached.canonicalize().unwrap()
        );
        fs::remove_file(&cached).unwrap();
        assert_eq!(prepare().unwrap(), ["model.safetensors", "config.json"]);
        assert!(
            weights
                .join("model.safetensors")
                .symlink_metadata()
                .is_err()
        );
        fs::write(weights.join("model.safetensors"), b"private download").unwrap();
        fs::remove_dir_all(runtime).unwrap();
        assert_eq!(fs::read(other).unwrap(), b"different revision");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gated_snapshot_reuses_exact_cached_files_without_a_transfer() {
        let root = env::temp_dir().join(format!("meshharbor-cache-gated-{}", Uuid::new_v4()));
        let home = root.join("home");
        let hub = root.join("shared");
        let revision = "reviewed";
        let directory = repo_directory(DINO_REPO);
        let snapshot = hub_snapshot(&hub, &directory, revision);
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&snapshot).unwrap();
        fs::write(snapshot.join("model.safetensors"), b"gated fixture").unwrap();
        download_snapshot(
            &root,
            &home,
            Some(&hub),
            (DINO_REPO, revision),
            vec!["model.safetensors".into()],
            "1",
            Default::default(),
        )
        .unwrap();
        assert!(
            hub_snapshot(&home.join(".cache/huggingface/hub"), &directory, revision)
                .join("model.safetensors")
                .symlink_metadata()
                .unwrap()
                .is_symlink()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn linked_destination_directory_is_rejected_before_linking_or_downloading() {
        let root =
            env::temp_dir().join(format!("meshharbor-cache-linked-parent-{}", Uuid::new_v4()));
        let runtime = root.join("runtime");
        let shared = root.join("shared");
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&shared).unwrap();
        symlink(&shared, runtime.join("weights")).unwrap();
        assert!(
            link_snapshot_files(
                &runtime,
                &runtime.join("weights"),
                Some(&shared),
                (DINO_REPO, "reviewed"),
                &["model.safetensors".into()],
                false,
                Default::default(),
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(&shared).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_shared_naf_is_left_untouched_and_not_reused() {
        let root = env::temp_dir().join(format!("meshharbor-cache-naf-{}", Uuid::new_v4()));
        let runtime = root.join("runtime");
        let hub = root.join("hub");
        let cached = hub.join("pixal3d-cuda/naf_release.pth");
        let target = runtime.join("weights/naf_release.pth");
        fs::create_dir_all(cached.parent().unwrap()).unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&cached, b"invalid cached weights").unwrap();
        symlink(&cached, &target).unwrap();
        link_cached_naf(&runtime, &target, Some(&hub), Default::default()).unwrap();
        assert!(target.symlink_metadata().is_err());
        assert_eq!(fs::read(cached).unwrap(), b"invalid cached weights");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn partial_snapshot_passes_only_missing_pinned_files_to_hf() {
        let root = env::temp_dir().join(format!("meshharbor-cache-partial-{}", Uuid::new_v4()));
        let runtime = root.join("runtime");
        let home = runtime.join("home");
        let shared = root.join("shared");
        let snapshot = pixal_snapshot(&shared, PIXAL_REVISION);
        let broken = format!("ckpts/{}", PIXAL_CHECKPOINTS[0]);
        let unreadable = root.join("outside-cache");
        fs::create_dir_all(snapshot.join("ckpts")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(&unreadable, b"not in the cache").unwrap();
        for file in pixal_files(false) {
            if file == broken {
                symlink(&unreadable, snapshot.join(file)).unwrap();
            } else {
                fs::write(snapshot.join(file), b"cached fixture").unwrap();
            }
        }

        let executable = runtime.join(".venv/bin/hf");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(
            &executable,
            b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/hf-args\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        download_pixal3d(&runtime, &home, Some(&shared), false, Default::default()).unwrap();
        let args = fs::read_to_string(home.join("hf-args")).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            [
                "download",
                PIXAL_REPO,
                "--revision",
                PIXAL_REVISION,
                &broken,
                "--max-workers",
                "2"
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
