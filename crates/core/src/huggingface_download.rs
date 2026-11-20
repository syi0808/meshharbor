use std::{
    env,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::Serialize;
use uuid::Uuid;

use crate::{CoreError, CoreResult, huggingface_browser::installed_browser_cli};

pub(crate) const DINO_REPO: &str = "facebook/dinov3-vitl16-pretrain-lvd1689m";
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
    hub.join(PIXAL_REPOSITORY).join("snapshots").join(revision)
}

fn present(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn shared_file(hub: &Path, revision: &str, relative: &str) -> Option<PathBuf> {
    let source = pixal_snapshot(hub, revision)
        .join(relative)
        .canonicalize()
        .ok()?;
    (present(&source)
        && (source.starts_with(hub.join(PIXAL_REPOSITORY))
            || source.starts_with(hub.join("blobs"))))
    .then_some(source)
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
            .and_then(|hub| shared_file(hub, PIXAL_REVISION, &relative))
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
    download_pixal_snapshot(
        runtime_root,
        home,
        shared_cache,
        PIXAL_REVISION,
        pixal_files(include_configs),
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
    download_pixal_snapshot(
        runtime_root,
        home,
        shared_hub_cache().as_deref(),
        PIXAL_MULTIVIEW_REVISION,
        PIXAL_MULTIVIEW_FILES
            .iter()
            .map(ToString::to_string)
            .collect(),
        execution,
    )
}

fn download_pixal_snapshot(
    runtime_root: &Path,
    home: &Path,
    shared_cache: Option<&Path>,
    revision: &str,
    files: Vec<String>,
    execution: crate::job_execution::JobExecution<'_>,
) -> CoreResult<()> {
    let local = pixal_snapshot(&home.join(".cache/huggingface/hub"), revision);
    let shared = shared_cache.and_then(|path| path.canonicalize().ok());
    let mut missing = Vec::new();
    let mut reused = 0;
    for relative in files {
        let target = local.join(&relative);
        if present(&target) {
            continue;
        }
        if let Some(source) = shared
            .as_deref()
            .and_then(|hub| shared_file(hub, revision, &relative))
        {
            if reuse_file(home, &source, &target)? {
                reused += 1;
                continue;
            }
        }
        missing.push(relative);
    }
    if missing.is_empty() {
        execution.note("All pinned Pixal3D files are cached; skipping their transfer.")?;
        return Ok(());
    }
    execution.note(&format!(
        "Reused {reused} files from the shared cache; downloading {} missing Pixal3D files.",
        missing.len()
    ))?;
    download(
        runtime_root,
        home,
        PIXAL_REPO,
        revision,
        missing
            .iter()
            .map(String::as_str)
            .chain(["--max-workers", "2"]),
        execution,
    )
}

fn reuse_file(home: &Path, source: &Path, target: &Path) -> io::Result<bool> {
    let parent = target.parent().expect("pinned file has a parent");
    let relative = parent
        .strip_prefix(home)
        .expect("pinned cache is inside the runtime home");
    let mut directory = home.to_path_buf();
    for component in relative.components() {
        directory.push(component);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if !metadata.is_dir() => return Ok(false),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&directory)?,
            Err(error) => return Err(error),
        }
    }
    match fs::hard_link(source, target) {
        Ok(()) => return Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(present(target)),
        Err(_) => {}
    }
    let temporary = parent.join(format!(".meshharbor-copy-{}", Uuid::new_v4()));
    let copied = fs::copy(source, &temporary).and_then(|_| fs::rename(&temporary, target));
    if copied.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    copied.map(|_| true)
}

pub(crate) fn download(
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
        fs::remove_dir_all(home.join(".cache")).unwrap();
        assert_eq!(
            fs::read(snapshot.join("pipeline.json")).unwrap(),
            b"cached fixture"
        );
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
