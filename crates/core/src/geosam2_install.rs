use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    CoreError, CoreResult, Pixal3dCudaRuntimeManager,
    external_installer::Installer,
    huggingface_download::{link_snapshot_files, shared_hub_cache},
    job_execution::JobExecution,
    model_environment::ModelEnvironment,
    model_storage::{
        managed_directory, managed_file_size, managed_weight_size, model_path,
        remove_managed_directories, weight_matches_sha,
    },
};

const MANAGED: &str = "runtimes/geosam2-managed";
const OWNER: &str = ".meshharbor-managed";
const STAMP: &str = ".environment-verified";
const ENVIRONMENT_VERSION: &[u8] = b"geosam2-environment-v1\n";
const REPO: &str = "https://github.com/VAST-AI-Research/GeoSAM2";
const REVISION: &str = "b5de23c60ab487d407b623d394a1614f9714761c";
const SEGMENT_ANYTHING_MLX_REPO: &str = "https://github.com/eisneim/sam2.1_mlx";
const SEGMENT_ANYTHING_MLX_REVISION: &str = "036f2d02a9069871fd2bf47109ff3b522cd222c4";
const WEIGHTS_DIR: &str = "runtimes/geosam2-managed/weights";
const WEIGHTS_NAME: &str = "geosam2.pt";
const WEIGHTS_BYTES: u64 = 615_627_435;
const WEIGHTS_SHA: &str = "2e391c0d9455fe92b69d61cdc94754d9b8081b9b541d09e1b6a3a55ebf6c6de0";
const WEIGHTS_REPO: &str = "VAST-AI/GeoSAM2";
const WEIGHTS_REVISION: &str = "ba92f5f50418f2fe9af1078448b63176df13b1ee";
const WORKER_FILES: &[(&str, &[u8])] = &[
    (
        "repart.py",
        include_bytes!("../../../runtimes/geosam2/repart.py"),
    ),
    (
        "repart_glb.py",
        include_bytes!("../../../runtimes/geosam2/repart_glb.py"),
    ),
    (
        "mlx_backend.py",
        include_bytes!("../../../runtimes/geosam2/mlx_backend.py"),
    ),
];
const DEPENDENCIES: &[&str] = &[
    "hydra-core==1.3.8",
    "omegaconf==2.3.1",
    "iopath==0.1.10",
    "pillow==13.0.0",
    "opencv-python-headless==5.1.0.95",
    "OpenEXR==3.5.5",
    "matplotlib==3.11.2",
    "tqdm==4.70.1",
    "trimesh==5.2.2",
    "pandas==3.1.1",
    "ninja==1.13.2",
    "numpy==2.5.5",
];

#[derive(Clone, Copy)]
pub(crate) enum Profile {
    Mlx,
    Cuda,
}

pub(crate) const PROFILES: [Profile; 2] = [Profile::Mlx, Profile::Cuda];

impl Profile {
    pub(crate) fn find(id: &str) -> Option<Self> {
        PROFILES.into_iter().find(|profile| profile.id() == id)
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Mlx => "geosam2-mlx",
            Self::Cuda => "geosam2-cuda",
        }
    }

    fn relative(self) -> String {
        format!("{MANAGED}/{}", self.id())
    }

    fn supported(self, cuda_available: bool) -> bool {
        match self {
            Self::Mlx => cfg!(all(target_os = "macos", target_arch = "aarch64")),
            Self::Cuda => cuda_available,
        }
    }

    pub(crate) fn require_supported(self, root: &Path) -> CoreResult<()> {
        let cuda_available =
            matches!(self, Self::Cuda) && Pixal3dCudaRuntimeManager::new(root).host_supported();
        if self.supported(cuda_available) {
            Ok(())
        } else {
            Err(CoreError::OperationUnavailable(format!(
                "{} is not supported on this host.",
                self.id()
            )))
        }
    }

    fn installed(self, root: &Path) -> CoreResult<bool> {
        let relative = self.relative();
        Ok(owned(root)?
            && managed_directory(root, &format!("{relative}/.venv"))?
            && root.join(&relative).join(".venv/bin/python").is_file()
            && managed_file_size(root, &format!("{relative}/source/sam2/build_sam.py"))?.is_some()
            && (!matches!(self, Self::Mlx)
                || managed_file_size(root, &format!("{relative}/sam2-mlx/src/sam2/build.py"))?
                    .is_some())
            && matches_file(root, &format!("{relative}/{STAMP}"), ENVIRONMENT_VERSION)?
            && WORKER_FILES.iter().all(|(name, contents)| {
                matches_file(root, &format!("{relative}/worker/{name}"), contents).unwrap_or(false)
            }))
    }

    pub(crate) fn status(self, root: &Path, cuda_available: bool) -> Value {
        let supported = self.supported(cuda_available);
        let installed = supported && self.installed(root).unwrap_or(false);
        let weights = weight_size(root);
        let downloaded = supported && matches!(weights, Ok(Some(WEIGHTS_BYTES)));
        let relative = self.relative();
        let runtime_root = model_path(root.join(relative));
        let model_paths = if downloaded {
            vec![model_path(weights_path(root))]
        } else {
            vec![]
        };
        json!({
            "id": self.id(),
            "status": if !supported { "unsupported_host" } else if installed && downloaded {
                "files_installed"
            } else { "installation_available" },
            "ready": false,
            "installable": supported,
            "environment_ready": installed,
            "weights_included": false,
            "models_downloaded": downloaded,
            "runtime_root": installed.then_some(&runtime_root),
            "python": installed.then(|| runtime_root.join(".venv/bin/python")),
            "model_paths": model_paths,
            "local_files": {
                "remove_models_available": supported && weights.is_ok_and(|size| size.is_some()),
            },
        })
    }

    pub(crate) fn install(self, root: &Path, execution: JobExecution<'_>) -> CoreResult<Value> {
        self.require_supported(root)?;
        execution.stage("prepare_runtime")?;
        let root = fs::canonicalize(root)?;
        let environment = ModelEnvironment::require(&root)?;
        let runtime = root.join(self.relative());
        if !self.installed(&root)? {
            ensure_owned(&root)?;
            managed_directory(&root, &self.relative())?;
            fs::create_dir_all(&runtime)?;
            for directory in ["source", "sam2-mlx", "worker", ".venv", "home", "uv-cache"] {
                managed_directory(&root, &format!("{}/{directory}", self.relative()))?;
            }
            fs::create_dir_all(runtime.join("home"))?;
            let installer = Installer::new(&runtime, &environment, execution);
            let source = runtime.join("source");
            let mlx_source = runtime.join("sam2-mlx");
            execution.stage("prepare_source")?;
            installer.checkout(&source, REPO, REVISION, false)?;
            if matches!(self, Self::Mlx) {
                installer.checkout(
                    &mlx_source,
                    SEGMENT_ANYTHING_MLX_REPO,
                    SEGMENT_ANYTHING_MLX_REVISION,
                    false,
                )?;
            }
            let worker = runtime.join("worker");
            fs::create_dir_all(&worker)?;
            for (name, bytes) in WORKER_FILES {
                managed_file_size(&root, &format!("{}/worker/{name}", self.relative()))?;
                fs::write(worker.join(name), bytes)?;
            }
            execution.stage("install_dependencies")?;
            installer.venv(&runtime.join(".venv"), "3.12")?;
            let python = runtime.join(".venv/bin/python");
            match self {
                Self::Mlx => installer.pip(
                    &python,
                    &["torch==2.15.0", "torchvision==0.29.2", "mlx==0.31.2"],
                )?,
                Self::Cuda => installer.pip(
                    &python,
                    &[
                        "torch==2.6.0",
                        "torchvision==0.21.0",
                        "--index-url",
                        "https://download.pytorch.org/whl/cu124",
                    ],
                )?,
            }
            installer.pip(&python, DEPENDENCIES)?;
            execution.stage("verify_environment")?;
            let mut paths = vec![worker, source];
            if matches!(self, Self::Mlx) {
                paths.push(mlx_source);
            }
            let check_device = match self {
                Self::Mlx => {
                    "import mlx.core as mx; from mlx_backend import Network; mx.set_default_device(mx.gpu); mx.eval(mx.ones(1) + 1)"
                }
                Self::Cuda => {
                    "import torch; assert torch.cuda.is_available(), 'NVIDIA CUDA unavailable to PyTorch'; torch.ones(1, device='cuda').sum().item()"
                }
            };
            let mut command = installer.command(&python);
            command
                .env("PYTHONPATH", std::env::join_paths(paths).map_err(|error| CoreError::InvalidInput(error.to_string()))?)
                .args(["-c", &format!(
                    "from repart import configure_upstream; configure_upstream(); import inference; {check_device}"
                )]);
            execution.visible(command, "GeoSAM2 backend preflight failed")?;
            managed_file_size(&root, &format!("{}/{STAMP}", self.relative()))?;
            execution.checkpoint()?;
            fs::write(runtime.join(STAMP), ENVIRONMENT_VERSION)?;
        }
        let token = crate::Pixal3dProfile::for_accelerator(self.id()).hf_token(&root);
        let file = install_weights(&root, &environment, &token, execution)?;
        Ok(json!({
            "backend_id": self.id(), "status": "files_installed",
            "runtime_root": runtime, "model_paths": [file],
        }))
    }
}

fn install_weights(
    root: &Path,
    environment: &ModelEnvironment,
    token: &Path,
    execution: JobExecution<'_>,
) -> CoreResult<PathBuf> {
    let file = weights_path(root);
    if weight_size(root)? == Some(WEIGHTS_BYTES) {
        execution.stage("verify_checkpoint")?;
        if verify_sha(&file, execution)? {
            return Ok(file);
        }
    }
    execution.stage("download_models")?;
    let directory = root.join(WEIGHTS_DIR);
    fs::create_dir_all(&directory)?;
    let staged = StagedDownload(directory.join(format!(".download-{}", Uuid::new_v4())));
    fs::create_dir(&staged.0)?;
    let staged_file = staged.0.join(WEIGHTS_NAME);
    let files = [WEIGHTS_NAME.to_string()];
    let linked = if link_snapshot_files(
        &directory,
        &staged.0,
        shared_hub_cache().as_deref(),
        (WEIGHTS_REPO, WEIGHTS_REVISION),
        &files,
        false,
        execution,
    )?
    .is_empty()
    {
        execution.stage("verify_checkpoint")?;
        match verify_sha(&staged_file, execution) {
            Ok(true) => true,
            Ok(false) | Err(CoreError::Io(_)) => {
                fs::remove_file(&staged_file)?;
                false
            }
            Err(error) => return Err(error),
        }
    } else {
        false
    };
    if linked {
        execution.note("Linked the SHA-256-verified GeoSAM2 checkpoint from the shared cache.")?;
    } else {
        execution.stage("download_models")?;
        let mut download = Command::new(environment.hf_cli());
        download
            .env_clear()
            .env("HOME", &staged.0)
            .env("HF_HOME", &staged.0)
            .env("HF_TOKEN_PATH", token)
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("HF_HUB_DISABLE_UPDATE_CHECK", "1")
            .args([
                "download",
                WEIGHTS_REPO,
                WEIGHTS_NAME,
                "--revision",
                WEIGHTS_REVISION,
                "--local-dir",
            ])
            .arg(&staged.0);
        execution.visible(download, "Could not download the pinned GeoSAM2 checkpoint")?;
        execution.stage("verify_checkpoint")?;
        if !verify_sha(&staged_file, execution)? {
            return Err(CoreError::OperationUnavailable(
                "GeoSAM2 checkpoint failed size or SHA-256 verification.".into(),
            ));
        }
    }
    execution.begin_commit()?;
    fs::rename(&staged_file, &file)?;
    Ok(file)
}

fn weights_path(root: &Path) -> PathBuf {
    root.join(WEIGHTS_DIR).join(WEIGHTS_NAME)
}

fn matches_file(root: &Path, relative: &str, contents: &[u8]) -> CoreResult<bool> {
    Ok(
        managed_file_size(root, relative)? == Some(contents.len() as u64)
            && fs::read(root.join(relative))? == contents,
    )
}

fn owned(root: &Path) -> CoreResult<bool> {
    if !managed_directory(root, MANAGED)? {
        return Ok(false);
    }
    if !matches_file(root, &format!("{MANAGED}/{OWNER}"), b"geosam2\n")? {
        return Err(CoreError::OperationUnavailable(
            "The GeoSAM2 directory is not owned by MeshHarbor. Move it aside before installing."
                .into(),
        ));
    }
    Ok(true)
}

fn ensure_owned(root: &Path) -> CoreResult<()> {
    if !owned(root)? {
        fs::create_dir_all(root.join("runtimes"))?;
        let directory = root.join(MANAGED);
        fs::create_dir(&directory)?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(OWNER))?
            .write_all(b"geosam2\n")?;
    }
    Ok(())
}

fn weight_size(root: &Path) -> CoreResult<Option<u64>> {
    if !owned(root)? {
        return Ok(None);
    }
    managed_weight_size(root, &format!("{WEIGHTS_DIR}/{WEIGHTS_NAME}"))
}

pub(crate) fn remove_weights(root: &Path) -> CoreResult<()> {
    if owned(root)? {
        weight_size(root)?;
        remove_managed_directories(root, &[WEIGHTS_DIR])?;
    }
    Ok(())
}

fn verify_sha(path: &Path, execution: JobExecution<'_>) -> CoreResult<bool> {
    let metadata = path.metadata()?;
    Ok(metadata.is_file()
        && metadata.len() == WEIGHTS_BYTES
        && weight_matches_sha(path, WEIGHTS_SHA, execution)?)
}

struct StagedDownload(PathBuf);

impl Drop for StagedDownload {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn test_directory() -> PathBuf {
        let root = std::env::temp_dir().join(format!("meshharbor-geosam2-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn shares_checkpoints_without_marking_repart_executable() {
        let directory = test_directory();
        let root = directory.as_path();
        assert_eq!(weight_size(root).unwrap(), None);
        ensure_owned(root).unwrap();
        fs::create_dir_all(root.join(WEIGHTS_DIR)).unwrap();
        File::create(weights_path(root))
            .unwrap()
            .set_len(WEIGHTS_BYTES)
            .unwrap();
        let cuda = Profile::Cuda.status(root, true);
        assert_eq!(cuda["models_downloaded"], true);
        assert_eq!(cuda["environment_ready"], false);
        assert_eq!(cuda["ready"], false);
        assert_eq!(cuda["model_paths"][0], json!(weights_path(root)));
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            let mlx = Profile::Mlx.status(root, true);
            assert_eq!(mlx["model_paths"], cuda["model_paths"]);
        }
        remove_weights(root).unwrap();
        assert_eq!(weight_size(root).unwrap(), None);
        assert!(owned(root).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_unowned_installation_directories() {
        let directory = test_directory();
        let root = directory.as_path();
        fs::create_dir_all(root.join(WEIGHTS_DIR)).unwrap();
        fs::write(weights_path(root), b"unmanaged data").unwrap();
        assert!(ensure_owned(root).is_err());
        assert!(remove_weights(root).is_err());
        assert_eq!(fs::read(weights_path(root)).unwrap(), b"unmanaged data");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn removal_does_not_follow_linked_weights() {
        let root = test_directory();
        let external = root.join("external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join(WEIGHTS_NAME), b"unmanaged data").unwrap();
        ensure_owned(&root).unwrap();
        std::os::unix::fs::symlink(&external, root.join(WEIGHTS_DIR)).unwrap();
        assert!(remove_weights(&root).is_err());
        assert_eq!(
            fs::read(external.join(WEIGHTS_NAME)).unwrap(),
            b"unmanaged data"
        );
        fs::remove_file(root.join(WEIGHTS_DIR)).unwrap();
        fs::create_dir(root.join(WEIGHTS_DIR)).unwrap();
        std::os::unix::fs::symlink(external.join(WEIGHTS_NAME), weights_path(&root)).unwrap();
        assert_eq!(weight_size(&root).unwrap(), Some(14));
        remove_weights(&root).unwrap();
        assert_eq!(weight_size(&root).unwrap(), None);
        assert_eq!(
            fs::read(external.join(WEIGHTS_NAME)).unwrap(),
            b"unmanaged data"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
