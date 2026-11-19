use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    CoreError, CoreResult, GenerationOptions, generation::run_generation_command,
    generation_catalog::generation_profile, job_execution::JobExecution,
};

pub(crate) struct ExternalProfile {
    pub id: &'static str,
    pub accelerator: &'static str,
    pub textured: bool,
    repository: &'static str,
    revision: &'static str,
    entry: &'static str,
    notice: &'static str,
}

pub(crate) const PROFILES: &[ExternalProfile] = &[
    ExternalProfile {
        id: "trellis2-mlx",
        accelerator: "Apple MLX",
        textured: true,
        repository: "https://github.com/gtrg55/trellis2-mlx",
        revision: "60a3b508177250f92edc8453fd451ab804abc028",
        entry: "mlx_backend/pipeline.py",
        notice: "TRELLIS.2 MLX requires separately authorized DINOv3 weights and review of the Metal dependencies.",
    },
    ExternalProfile {
        id: "trellis2-cuda",
        accelerator: "NVIDIA CUDA",
        textured: true,
        repository: "https://github.com/microsoft/TRELLIS.2",
        revision: "75fbf0183001ed9876c8dbb35de6b68552ee08bd",
        entry: "trellis2/pipelines/trellis2_image_to_3d.py",
        notice: "TRELLIS.2 CUDA uses gated DINOv3 and the official nvdiffrast baker; nvdiffrast v0.4.0 restricts use to non-commercial research or evaluation.",
    },
    ExternalProfile {
        id: "triposg-mlx",
        accelerator: "Apple MLX",
        textured: false,
        repository: "https://github.com/cavi-ai/mlx-agent",
        revision: "c4c04b4cbbdf563fa3ae931f9a584bb7f0b240d5",
        entry: "src/mlx_agent/resources/ports/mlx-3d/triposg/pipeline.py",
        notice: "The MLX port carries HunyuanDiT terms. Review both the port and the scope of the TripoSG checkpoint before use.",
    },
    ExternalProfile {
        id: "triposg-cuda",
        accelerator: "NVIDIA CUDA",
        textured: false,
        repository: "https://github.com/VAST-AI-Research/TripoSG",
        revision: "fc5c40990181e2a756c4e0b1c2f4d6b5202faf8c",
        entry: "triposg/pipelines/pipeline_triposg.py",
        notice: "The official FlashVDM path is not authorized by its community license in South Korea, the EU, or the UK, including non-commercial use. A separate grant is needed for covered use there.",
    },
];

const TRELLIS_CHECKPOINTS: &[&str] = &[
    "ckpts/ss_flow_img_dit_1_3B_64_bf16",
    "ckpts/shape_dec_next_dc_f16c32_fp16",
    "ckpts/slat_flow_img2shape_dit_1_3B_512_bf16",
    "ckpts/slat_flow_img2shape_dit_1_3B_1024_bf16",
    "ckpts/tex_dec_next_dc_f16c32_fp16",
    "ckpts/slat_flow_imgshape2tex_dit_1_3B_512_bf16",
    "ckpts/slat_flow_imgshape2tex_dit_1_3B_1024_bf16",
    "microsoft/TRELLIS-image-large/ckpts/ss_dec_conv3d_16l8_fp16",
];

const TRIPOSG_CUDA_FILES: &[&str] = &[
    "model_index.json",
    "feature_extractor_dinov2/preprocessor_config.json",
    "image_encoder_dinov2/config.json",
    "image_encoder_dinov2/model.safetensors",
    "scheduler/scheduler_config.json",
    "transformer/config.json",
    "transformer/diffusion_pytorch_model.safetensors",
    "vae/config.json",
    "vae/diffusion_pytorch_model.safetensors",
];

const TRIPOSG_MLX_FILES: &[&str] = &[
    "config.json",
    "image_encoder_dinov2.safetensors",
    "transformer.safetensors",
    "vae.safetensors",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Activation {
    source_revision: String,
    authorized_local_use: bool,
}

#[derive(Debug)]
struct Readiness {
    status: &'static str,
    missing: Vec<String>,
}

impl Readiness {
    fn ready(&self) -> bool {
        self.missing.is_empty()
    }
}

impl ExternalProfile {
    pub fn find(id: &str) -> Option<&'static Self> {
        PROFILES.iter().find(|profile| profile.id == id)
    }

    fn host_supported(&self) -> bool {
        matches!(
            (
                self.id.ends_with("-mlx"),
                std::env::consts::OS,
                std::env::consts::ARCH,
            ),
            (true, "macos", "aarch64") | (false, "linux", "x86_64")
        )
    }

    fn root(&self, data_root: &Path) -> PathBuf {
        data_root.join("runtimes").join(self.id)
    }

    fn required_model_files(&self) -> Vec<String> {
        if self.textured {
            let mut paths = vec![
                "model/pipeline.json".into(),
                "dinov3/config.json".into(),
                "dinov3/model.safetensors".into(),
            ];
            for checkpoint in TRELLIS_CHECKPOINTS {
                paths.push(format!("model/{checkpoint}.json"));
                paths.push(format!("model/{checkpoint}.safetensors"));
            }
            paths
        } else {
            let files = if self.id == "triposg-mlx" {
                TRIPOSG_MLX_FILES
            } else {
                TRIPOSG_CUDA_FILES
            };
            files.iter().map(|file| format!("model/{file}")).collect()
        }
    }

    fn check_local_files(&self, root: &Path) -> Readiness {
        let activation = read_activation(&root.join("activate.json"));
        match activation {
            Some(activation)
                if activation.authorized_local_use
                    && activation.source_revision == self.revision => {}
            _ => {
                return Readiness {
                    status: "manual_activation_required",
                    missing: vec![format!(
                        "Prepare the local runtime and confirm that you have permission for your intended use before creating {} for source revision {}.",
                        root.join("activate.json").display(),
                        self.revision
                    )],
                };
            }
        }

        let mut missing = Vec::new();
        for path in [
            root.join(".venv/bin/python"),
            root.join("source").join(self.entry),
        ] {
            if !is_nonempty_file(&path) {
                missing.push(format!("Missing local runtime file: {}", path.display()));
            }
        }
        for file in self.required_model_files() {
            let path = root.join("weights").join(file);
            if !is_nonempty_file(&path) {
                missing.push(format!("Missing local model file: {}", path.display()));
            }
        }
        Readiness {
            status: if missing.is_empty() {
                "ready_paths_unverified"
            } else {
                "manual_files_required"
            },
            missing,
        }
    }

    fn readiness(&self, root: &Path) -> Readiness {
        if self.host_supported() {
            self.check_local_files(root)
        } else {
            Readiness {
                status: "unsupported_host",
                missing: vec![format!(
                    "{} is not supported on this host.",
                    self.accelerator
                )],
            }
        }
    }

    pub fn is_ready(&self, data_root: &Path) -> bool {
        self.readiness(&self.root(data_root)).ready()
    }

    pub fn status(&self, data_root: &Path) -> Value {
        let root = self.root(data_root);
        let readiness = self.readiness(&root);
        json!({
            "id": self.id,
            "generation": generation_profile(self.id),
            "accelerator": self.accelerator,
            "manual_setup": true,
            "host_supported": self.host_supported(),
            "ready": readiness.ready(),
            "status": readiness.status,
            "validation_level": "operator_activation_and_required_paths_only",
            "runtime_root": root,
            "installable": false,
            "weights_included": false,
            "downloads_enabled": false,
            "gated_model_access_required": self.textured,
            "source": { "repository": self.repository, "revision": self.revision },
            "license_notice": self.notice,
            "missing": readiness.missing,
        })
    }

    pub fn require_ready(&self, data_root: &Path) -> CoreResult<()> {
        let readiness = self.readiness(&self.root(data_root));
        if readiness.ready() {
            return Ok(());
        }
        Err(CoreError::OperationUnavailable(format!(
            "{} is not ready: {} {} See docs/external-generation-runtimes.md for manual setup.",
            self.id,
            readiness.missing.join("; "),
            self.notice
        )))
    }

    pub fn generate(
        &self,
        data_root: &Path,
        input: &Path,
        output: &Path,
        options: &GenerationOptions,
        execution: JobExecution<'_>,
    ) -> CoreResult<u128> {
        self.require_ready(data_root)?;
        let root = self.root(data_root).canonicalize()?;
        let input = input.canonicalize()?;
        let work = output
            .parent()
            .expect("generation output always has a work directory")
            .canonicalize()?;
        let output = work.join(output.file_name().expect("generation output has a name"));
        let adapter = work.join("external-generation.py");
        fs::write(&adapter, include_bytes!("external_generation.py"))?;
        let home = root.join("home");
        fs::create_dir_all(&home)?;
        let bin = root.join(".venv/bin");
        let paths = std::env::var_os("PATH").unwrap_or_default();
        let path =
            std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&paths)))
                .map_err(|error| CoreError::OperationUnavailable(error.to_string()))?;
        let mut command = Command::new(bin.join("python"));
        command
            .current_dir(&work)
            .env_clear()
            .env("PATH", path)
            .env("HOME", &home)
            .env("HF_HOME", home.join("huggingface"))
            .env("HF_HUB_OFFLINE", "1")
            .env("HF_HUB_DISABLE_IMPLICIT_TOKEN", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .args(["-I", "-B", "-u"])
            .arg(adapter)
            .arg(self.id)
            .arg(root.join("source"))
            .arg(root.join("weights"))
            .arg(input)
            .arg(output)
            .arg(options.seed.to_string())
            .arg(options.resolution.to_string())
            .arg(options.texture_size.to_string())
            .arg(options.target_faces.to_string());
        if self.id.ends_with("-cuda") {
            for key in ["CUDA_HOME", "CUDA_VISIBLE_DEVICES"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
        }
        let started = Instant::now();
        let status = run_generation_command(&mut command, execution)?;
        if !status.success() {
            return Err(CoreError::OperationUnavailable(format!(
                "{} worker failed ({status}). See the engine stderr for details.",
                self.id
            )));
        }
        Ok(started.elapsed().as_millis())
    }
}

fn is_nonempty_file(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn read_activation(path: &Path) -> Option<Activation> {
    const MAX_ACTIVATION_BYTES: u64 = 4096;
    let metadata = path.symlink_metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_ACTIVATION_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_ACTIVATION_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_ACTIVATION_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
