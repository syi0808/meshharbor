use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::json;

use crate::{
    CoreError, CoreResult, Pixal3dCudaRuntimeManager,
    external_generation::{
        ExternalProfile, TRELLIS_CHECKPOINTS, TRELLIS_TEXT_CUDA_ID, TRELLIS_TEXT_MLX_ID,
        TRELLIS_TEXT_MODEL_FILES, TRIPOSG_CUDA_FILES,
    },
    huggingface_access::{DINO_REVISION, HuggingFaceAccess, check_model_access},
    job_execution::JobExecution,
    model_environment::ModelEnvironment,
    model_storage::{managed_directory, remove_managed_directories},
};

const OWNER_FILE: &str = ".meshharbor-managed";
const DINO_REPO: &str = "facebook/dinov3-vitl16-pretrain-lvd1689m";
const IMAGE_REPO: &str = "microsoft/TRELLIS-image-large";
const IMAGE_REVISION: &str = "25e0d31ffbebe4b5a97464dd851910efc3002d96";
const TRIPOSG_PORT: &str = "src/mlx_agent/resources/ports/mlx-3d";
const UTILS3D: &str =
    "git+https://github.com/EasternJournalist/utils3d.git@9a4eb15e4021b67b12c460c7057d642626897ec8";
const FLEXICUBES: &str = "815e075a2a400d06c48d94c347674344ed6ae5c5";

struct Snapshot {
    repo: &'static str,
    revision: &'static str,
    directory: &'static str,
    files: Vec<String>,
}

impl Snapshot {
    fn new(
        repo: &'static str,
        revision: &'static str,
        directory: &'static str,
        files: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            repo,
            revision,
            directory,
            files: files.into_iter().map(Into::into).collect(),
        }
    }
}

fn snapshots(id: &str) -> Vec<Snapshot> {
    match id {
        TRELLIS_TEXT_MLX_ID | TRELLIS_TEXT_CUDA_ID => [
            (
                "microsoft/TRELLIS-text-large",
                "4aad9f4a110329a410974d7f41ce5333a9a1fc87",
                "model",
            ),
            (IMAGE_REPO, IMAGE_REVISION, "decoders"),
            (
                "openai/clip-vit-large-patch14",
                "32bd64288804d66eefd0ccbe215aa642df71cc41",
                "clip",
            ),
        ]
        .into_iter()
        .map(|(repo, revision, directory)| {
            Snapshot::new(
                repo,
                revision,
                directory,
                TRELLIS_TEXT_MODEL_FILES.iter().filter_map(|file| {
                    let (parent, name) = file.split_once('/')?;
                    (parent == directory).then_some(name)
                }),
            )
        })
        .collect(),
        "trellis2-mlx" | "trellis2-cuda" => {
            let (shared, own) = TRELLIS_CHECKPOINTS
                .iter()
                .copied()
                .partition::<Vec<_>, _>(|name| name.starts_with("microsoft/"));
            let files = |names: Vec<&'static str>| {
                names.into_iter().flat_map(|name| {
                    let name = name
                        .strip_prefix("microsoft/TRELLIS-image-large/")
                        .unwrap_or(name);
                    [format!("{name}.json"), format!("{name}.safetensors")]
                })
            };
            vec![
                Snapshot::new(
                    "microsoft/TRELLIS.2-4B",
                    "af44b45f2e35a493886929c6d786e563ec68364d",
                    "model",
                    std::iter::once("pipeline.json".into()).chain(files(own)),
                ),
                Snapshot::new(
                    IMAGE_REPO,
                    IMAGE_REVISION,
                    "model/microsoft/TRELLIS-image-large",
                    files(shared),
                ),
                Snapshot::new(
                    DINO_REPO,
                    DINO_REVISION,
                    "dinov3",
                    ["config.json", "model.safetensors"],
                ),
            ]
        }
        "triposg-mlx" | "triposg-cuda" => vec![Snapshot::new(
            "VAST-AI/TripoSG",
            "2c1c516d22d58db486a058d98d31bb6177344e06",
            if id == "triposg-mlx" {
                "original"
            } else {
                "model"
            },
            TRIPOSG_CUDA_FILES.iter().copied(),
        )],
        _ => unreachable!("only catalog profiles have installers"),
    }
}

impl ExternalProfile {
    fn managed_relative(&self) -> String {
        format!("runtimes/{}-managed", self.id)
    }

    pub(crate) fn managed_root(&self, root: &Path) -> PathBuf {
        root.join(self.managed_relative())
    }

    pub(crate) fn check_owner(&self, root: &Path) -> CoreResult<bool> {
        if !managed_directory(root, &self.managed_relative())? {
            return Ok(false);
        }
        let marker = self.managed_root(root).join(OWNER_FILE);
        if !marker
            .symlink_metadata()
            .is_ok_and(|entry| entry.is_file() && entry.len() == self.id.len() as u64)
            || fs::read(&marker)? != self.id.as_bytes()
        {
            return Err(CoreError::OperationUnavailable(format!(
                "{} is not owned by MeshHarbor. Move it aside before installing.",
                self.managed_root(root).display()
            )));
        }
        Ok(true)
    }

    fn removable_paths(&self) -> [String; 2] {
        let relative = self.managed_relative();
        [
            format!("{relative}/weights"),
            format!("{relative}/home/.cache/huggingface/xet"),
        ]
    }

    pub(crate) fn removable_model_files(&self, root: &Path) -> CoreResult<bool> {
        if !self.check_owner(root)? {
            return Ok(false);
        }
        let mut present = false;
        for path in self.removable_paths() {
            present |= managed_directory(root, &path)?;
        }
        Ok(present)
    }

    pub(crate) fn remove_models(&self, root: &Path) -> CoreResult<()> {
        if self.check_owner(root)? {
            let paths = self.removable_paths();
            remove_managed_directories(root, &paths.each_ref().map(String::as_str))?;
        }
        Ok(())
    }

    pub(crate) fn install(
        &self,
        root: &Path,
        execution: JobExecution<'_>,
    ) -> CoreResult<serde_json::Value> {
        if !self.host_supported()
            || (self.id.ends_with("-cuda")
                && !Pixal3dCudaRuntimeManager::new(root).host_supported())
        {
            return Err(CoreError::OperationUnavailable(format!(
                "{} is not supported on this host.",
                self.accelerator
            )));
        }
        let root = fs::canonicalize(root)?;
        let runtime = self.managed_root(&root);
        execution.stage("prepare_runtime")?;
        if !self.check_owner(&root)? {
            fs::create_dir_all(root.join("runtimes"))?;
            fs::create_dir(&runtime)?;
            fs::write(runtime.join(OWNER_FILE), self.id)?;
        }
        self.removable_model_files(&root)?;
        for directory in ["source", ".venv", "home", "uv-cache"] {
            managed_directory(&root, &format!("{}/{directory}", self.managed_relative()))?;
        }
        fs::create_dir_all(runtime.join("home"))?;
        let environment = ModelEnvironment::require(&root)?;
        let installer = Installer::new(&runtime, &environment, execution);
        let version = if matches!(self.id, "triposg-mlx" | TRELLIS_TEXT_MLX_ID) {
            "3.12"
        } else {
            "3.10"
        };
        let token = crate::Pixal3dProfile::for_accelerator(self.id).hf_token(&root);
        if self.needs_dinov3() {
            execution.stage("check_model_access")?;
            let mut command = installer.command(environment.hf_python());
            command.env("HF_TOKEN_PATH", &token);
            match check_model_access(command)? {
                HuggingFaceAccess::Verified => {}
                HuggingFaceAccess::NotConnected => return Err(CoreError::OperationUnavailable(
                    "Sign in under Studio → Settings → Connections before installing TRELLIS.2.".into(),
                )),
                HuggingFaceAccess::AccessDenied => return Err(CoreError::OperationUnavailable(
                    "Request access to DINOv3 on Hugging Face with the connected account, then retry.".into(),
                )),
            }
        }
        execution.stage("prepare_source")?;
        installer.checkout(
            &runtime.join("source"),
            self.repository,
            self.revision,
            matches!(
                self.id,
                "trellis2-mlx" | "trellis2-cuda" | TRELLIS_TEXT_CUDA_ID
            ),
        )?;
        if self.id == TRELLIS_TEXT_MLX_ID {
            installer.checkout(
                &runtime.join("source/flexicubes"),
                "https://github.com/MaxtirError/FlexiCubes",
                FLEXICUBES,
                false,
            )?;
        }
        execution.stage("install_dependencies")?;
        installer.venv(&runtime.join(".venv"), version)?;
        installer.install_dependencies(self.id)?;
        execution.stage("verify_environment")?;
        installer.verify(self.id)?;
        execution.stage("download_models")?;
        let weights = runtime.join("weights");
        let model_files_present = || {
            self.required_model_files().iter().all(|file| {
                weights
                    .join(file)
                    .metadata()
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
            })
        };
        for snapshot in snapshots(self.id)
            .into_iter()
            .filter(|_| self.id != "triposg-mlx" || !model_files_present())
        {
            let relative = format!("{}/weights/{}", self.managed_relative(), snapshot.directory);
            managed_directory(&root, &relative)?;
            for file in &snapshot.files {
                if let Some((parent, _)) = file.rsplit_once('/') {
                    managed_directory(&root, &format!("{relative}/{parent}"))?;
                }
            }
            let mut command = installer.command(environment.hf_cli());
            command
                .env("HF_TOKEN_PATH", &token)
                .args(["download", snapshot.repo, "--revision", snapshot.revision])
                .args(&snapshot.files)
                .arg("--local-dir")
                .arg(weights.join(snapshot.directory));
            execution.visible(command, "Could not download pinned model files")?;
        }
        if self.id == "triposg-mlx" && !model_files_present() {
            execution.stage("convert_models")?;
            remove_managed_directories(
                &root,
                &[&format!("{}/weights/model", self.managed_relative())],
            )?;
            let mut command = installer.command(runtime.join(".venv/bin/python"));
            command
                .args(["-I", "-B"])
                .arg(
                    runtime
                        .join("source")
                        .join(TRIPOSG_PORT)
                        .join("triposg/convert.py"),
                )
                .arg("--hf-path")
                .arg(weights.join("original"))
                .arg("--mlx-path")
                .arg(weights.join("model"));
            execution.visible(command, "TripoSG MLX weight conversion failed")?;
        }
        if self.id == "triposg-mlx" {
            remove_managed_directories(
                &root,
                &[&format!("{}/weights/original", self.managed_relative())],
            )?;
        }
        execution.stage("verify_models")?;
        for file in self.required_model_files() {
            if !weights
                .join(&file)
                .metadata()
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
            {
                return Err(CoreError::OperationUnavailable(format!(
                    "Model installation did not produce {file}."
                )));
            }
        }
        let activation = json!({"source_revision": self.revision, "authorized_local_use": true});
        let staged = runtime.join(format!(".activation-{}", uuid::Uuid::new_v4()));
        fs::write(&staged, activation.to_string())?;
        fs::rename(staged, runtime.join("activate.json"))?;
        Ok(json!({"backend_id": self.id, "status": "ready", "runtime_root": runtime}))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::external_generation::PROFILES;

    #[test]
    fn pinned_downloads_cover_every_generation_file() {
        for profile in PROFILES {
            let mut downloaded = BTreeSet::new();
            for snapshot in snapshots(profile.id) {
                assert_eq!(snapshot.revision.len(), 40);
                assert!(
                    snapshot
                        .revision
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                );
                assert!(!snapshot.files.is_empty());
                assert_eq!(snapshot.repo == DINO_REPO, snapshot.directory == "dinov3");
                for file in snapshot.files {
                    assert!(downloaded.insert(format!("{}/{file}", snapshot.directory)));
                }
            }
            let expected = if profile.id == "triposg-mlx" {
                TRIPOSG_CUDA_FILES
                    .iter()
                    .map(|file| format!("original/{file}"))
                    .collect()
            } else {
                profile.required_model_files().into_iter().collect()
            };
            assert_eq!(downloaded, expected, "{}", profile.id);
        }
    }

    #[test]
    fn model_removal_keeps_environment_and_existing_operator_install() {
        let root = env::temp_dir().join(format!("meshharbor-install-{}", uuid::Uuid::new_v4()));
        let profile = ExternalProfile::find(TRELLIS_TEXT_MLX_ID).unwrap();
        let legacy = root.join("runtimes/trellis-text-mlx/weights");
        let managed = profile.managed_root(&root);
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(managed.join("weights")).unwrap();
        fs::create_dir(managed.join(".venv")).unwrap();
        fs::create_dir_all(managed.join("home/.cache/huggingface/xet")).unwrap();
        assert!(profile.remove_models(&root).is_err());
        assert!(managed.join("weights").exists());
        fs::write(managed.join(OWNER_FILE), profile.id).unwrap();
        assert!(profile.removable_model_files(&root).unwrap());
        profile.remove_models(&root).unwrap();
        assert!(!managed.join("weights").exists());
        assert!(!managed.join("home/.cache/huggingface/xet").exists());
        assert!(managed.join(".venv").exists());
        assert!(legacy.exists());
        assert!(!profile.removable_model_files(&root).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}

pub(crate) struct Installer<'a> {
    runtime: &'a Path,
    environment: &'a ModelEnvironment,
    execution: JobExecution<'a>,
}

impl<'a> Installer<'a> {
    pub(crate) fn new(
        runtime: &'a Path,
        environment: &'a ModelEnvironment,
        execution: JobExecution<'a>,
    ) -> Self {
        Self {
            runtime,
            environment,
            execution,
        }
    }

    pub(crate) fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let home = self.runtime.join("home");
        let bin = self.runtime.join(".venv/bin");
        let mut command = Command::new(program);
        let path = env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        let path =
            env::join_paths(std::iter::once(bin).chain(env::split_paths(&path))).unwrap_or(path);
        command
            .env_clear()
            .current_dir(self.runtime)
            .stdin(Stdio::null())
            .env("PATH", path)
            .env("HOME", &home)
            .env("UV_NO_CONFIG", "1")
            .env("UV_PYTHON_DOWNLOADS", "never")
            .env("UV_CACHE_DIR", self.runtime.join("uv-cache"))
            .env("HF_HOME", home.join(".cache/huggingface"))
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("HF_HUB_DISABLE_UPDATE_CHECK", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        for key in [
            "TMPDIR",
            "CUDA_HOME",
            "CUDA_VISIBLE_DEVICES",
            "TORCH_CUDA_ARCH_LIST",
            "SDKROOT",
            "DEVELOPER_DIR",
        ] {
            if let Some(value) = env::var_os(key) {
                command.env(key, value);
            }
        }
        command
    }

    pub(crate) fn checkout(
        &self,
        path: &Path,
        repo: &str,
        revision: &str,
        submodules: bool,
    ) -> CoreResult<()> {
        let mut init = self.command("git");
        init.arg("init").arg(path);
        self.execution
            .visible(init, "Could not prepare pinned source checkout")?;
        let git = || {
            let mut command = self.command("git");
            command.arg("-C").arg(path).args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "protocol.file.allow=never",
            ]);
            command
        };
        let mut fetch = git();
        fetch.args(["fetch", "--depth=1", repo, revision]);
        self.execution
            .visible(fetch, "Could not fetch pinned source revision")?;
        let mut checkout = git();
        checkout.args(["checkout", "--detach", revision]);
        self.execution
            .visible(checkout, "Could not check out pinned source revision")?;
        if submodules {
            let mut update = git();
            update.args([
                "-c",
                &format!("remote.origin.url={repo}"),
                "submodule",
                "update",
                "--init",
                "--recursive",
                "--depth",
                "1",
            ]);
            self.execution
                .visible(update, "Could not prepare pinned source submodules")?;
        }
        Ok(())
    }

    pub(crate) fn venv(&self, destination: &Path, version: &str) -> CoreResult<()> {
        let mut command = self.command(self.environment.uv());
        command
            .args(["venv", "--no-project", "--allow-existing", "--python"])
            .arg(self.environment.python(version))
            .arg(destination);
        self.execution
            .visible(command, "Could not prepare managed Python")
    }

    pub(crate) fn pip(&self, python: &Path, args: &[&str]) -> CoreResult<()> {
        let mut command = self.command(self.environment.uv());
        command
            .args(["pip", "install", "--python"])
            .arg(python)
            .args(args);
        self.execution.visible(
            command,
            "Could not install the managed backend dependencies",
        )
    }

    fn install_dependencies(&self, id: &str) -> CoreResult<()> {
        let python = self.runtime.join(".venv/bin/python");
        if id.ends_with("-cuda") {
            let torch = if id == TRELLIS_TEXT_CUDA_ID {
                &[
                    "torch==2.4.0",
                    "torchvision==0.19.0",
                    "xformers==0.0.27.post2",
                    "--index-url",
                    "https://download.pytorch.org/whl/cu121",
                ][..]
            } else {
                &[
                    "torch==2.6.0",
                    "torchvision==0.21.0",
                    "--index-url",
                    "https://download.pytorch.org/whl/cu124",
                ][..]
            };
            self.pip(&python, torch)?;
        }
        match id {
            TRELLIS_TEXT_MLX_ID => self.pip(
                &python,
                &[
                    "mlx==0.31.2",
                    "numpy==2.5.5",
                    "pillow==13.0.0",
                    "tokenizers==0.21.4",
                    "trimesh==5.2.1",
                    "fast-simplification==0.1.13",
                ],
            ),
            TRELLIS_TEXT_CUDA_ID => {
                self.pip(
                    &python,
                    &[
                        "numpy<2",
                        "scipy",
                        "pillow",
                        "opencv-python-headless==4.11.0.86",
                        "easydict",
                        "tqdm",
                        "plyfile",
                        "open3d==0.19.0",
                        "rembg[cpu]==2.0.65",
                        "trimesh==4.9.0",
                        "fast-simplification==0.1.13",
                        "transformers==4.48.3",
                        "safetensors",
                        "spconv-cu120==2.3.6",
                        UTILS3D,
                    ],
                )?;
                self.pip(
                    &python,
                    &[
                        "kaolin==0.16.0",
                        "--find-links",
                        "https://nvidia-kaolin.s3.us-east-2.amazonaws.com/torch-2.4.0_cu121.html",
                    ],
                )
            }
            "triposg-mlx" => self.pip(
                &python,
                &[
                    "mlx==0.31.1",
                    "numpy",
                    "pillow",
                    "scikit-image",
                    "trimesh",
                    "fast-simplification",
                ],
            ),
            "triposg-cuda" => {
                self.pip(
                    &python,
                    &[
                        "diffusers==0.32.2",
                        "transformers==4.48.3",
                        "einops",
                        "huggingface-hub<1",
                        "safetensors",
                        "opencv-python-headless<4.12",
                        "trimesh",
                        "omegaconf",
                        "scikit-image",
                        "numpy<2",
                        "peft==0.14.0",
                        "jaxtyping",
                        "typeguard",
                        "pillow",
                        "fast-simplification",
                        "setuptools",
                        "wheel",
                        "ninja",
                    ],
                )?;
                self.pip(&python, &["--no-build-isolation", "diso==0.1.4"])
            }
            "trellis2-mlx" | "trellis2-cuda" => {
                self.pip(
                    &python,
                    &[
                        "numpy<2",
                        "opencv-python-headless<4.12",
                        "transformers>=4.40,<5",
                        "huggingface-hub<1",
                        "safetensors",
                        "pillow",
                        "easydict",
                        "plyfile",
                        "zstandard",
                        "kornia",
                        "timm",
                        "trimesh",
                        "xatlas",
                        "fast-simplification",
                        "pygltflib",
                        "tqdm",
                        "imageio",
                        "imageio-ffmpeg",
                        "scipy",
                        "scikit-image",
                        "einops",
                        "setuptools",
                        "wheel",
                        "ninja",
                        UTILS3D,
                    ],
                )?;
                let native = if id == "trellis2-mlx" {
                    self.pip(
                        &python,
                        &["torch==2.11.0", "torchvision==0.26.0", "mlx==0.31.2"],
                    )?;
                    &[
                        "git+https://github.com/pedronaugusto/mtldiffrast.git@db3186be168a495f43ea095f7311a18a4de0a74c",
                        "git+https://github.com/pedronaugusto/mtlmesh.git@4a46faae8ab6def6f308c499e22a23020cdacd5b",
                        "git+https://github.com/pedronaugusto/mtlgemm.git@c2e5d3fa15152bc4aef65773cab1ded5a1855d38",
                    ]
                } else {
                    &[
                        "git+https://github.com/JeffreyXiang/CuMesh.git@12289e1062f0603f2f0d0771b02e1395d247f26f",
                        "git+https://github.com/JeffreyXiang/FlexGEMM.git@6dd94a859c26ee8246888502eada3dd8ad85532e",
                        "git+https://github.com/NVlabs/nvdiffrast.git@253ac4fcea7de5f396371124af597e6cc957bfae",
                    ]
                };
                for package in native {
                    self.pip(&python, &["--no-build-isolation", package])?;
                }
                let local = self.runtime.join("source/o-voxel");
                self.pip(&python, &["--no-build-isolation", &local.to_string_lossy()])
            }
            _ => unreachable!("only catalog profiles have installers"),
        }
    }

    fn verify(&self, id: &str) -> CoreResult<()> {
        let imports = match id {
            TRELLIS_TEXT_MLX_ID => {
                "import mlx.core as mx, tokenizers, trimesh; assert mx.metal.is_available()"
            }
            TRELLIS_TEXT_CUDA_ID => {
                "import torch, kaolin, spconv.pytorch, xformers.ops, trellis; assert torch.cuda.is_available()"
            }
            "trellis2-mlx" => {
                "import mlx.core as mx, torch, mtldiffrast, o_voxel, cumesh, flex_gemm; from mlx_backend.pipeline import create_mlx_pipeline; assert mx.metal.is_available() and torch.backends.mps.is_available()"
            }
            "trellis2-cuda" => {
                "import torch, o_voxel, cumesh, flex_gemm, nvdiffrast.torch; from trellis2.pipelines import Trellis2ImageTo3DPipeline; assert torch.cuda.is_available()"
            }
            "triposg-mlx" => {
                "import mlx.core as mx; from triposg import pipeline; assert mx.metal.is_available()"
            }
            "triposg-cuda" => {
                "import torch, diso; from triposg.pipelines.pipeline_triposg import TripoSGPipeline; assert torch.cuda.is_available()"
            }
            _ => unreachable!("only catalog profiles have installers"),
        };
        let mut source = self.runtime.join("source");
        if id == "triposg-mlx" {
            source = source.join(TRIPOSG_PORT);
        }
        let mut command = self.command(self.runtime.join(".venv/bin/python"));
        command
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env(
                "ATTN_BACKEND",
                if id == TRELLIS_TEXT_CUDA_ID {
                    "xformers"
                } else {
                    "sdpa"
                },
            )
            .env("SPARSE_BACKEND", "spconv")
            .env("SPCONV_ALGO", "native")
            .args([
                "-I",
                "-B",
                "-c",
                &format!("import sys; sys.path.insert(0, sys.argv[1]); {imports}"),
            ])
            .arg(source);
        self.execution
            .visible(command, "Managed backend import or GPU preflight failed")
    }
}
