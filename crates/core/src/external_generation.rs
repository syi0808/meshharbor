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
    CoreError, CoreResult, GenerationInputMode, GenerationOptions,
    generation::run_generation_command_with_input, generation_catalog::generation_profile,
    job_execution::JobExecution, model_storage::model_path,
};

pub(crate) const TRELLIS_TEXT_CUDA_ID: &str = "trellis-text-cuda";
pub(crate) const TRELLIS_TEXT_MLX_ID: &str = "trellis-text-mlx";

pub(crate) enum ExternalInput<'a> {
    Image(&'a Path),
    Text(&'a str),
}

pub(crate) struct ExternalProfile {
    pub id: &'static str,
    pub accelerator: &'static str,
    pub textured: bool,
    pub input_mode: GenerationInputMode,
    pub repository: &'static str,
    pub revision: &'static str,
    entry: &'static str,
    notice: &'static str,
}

pub(crate) const PROFILES: &[ExternalProfile] = &[
    ExternalProfile {
        id: "trellis2-mlx",
        accelerator: "Apple MLX",
        textured: true,
        input_mode: GenerationInputMode::Image,
        repository: "https://github.com/gtrg55/trellis2-mlx",
        revision: "60a3b508177250f92edc8453fd451ab804abc028",
        entry: "mlx_backend/pipeline.py",
        notice: "TRELLIS.2 MLX requires separately authorized DINOv3 weights and review of the Metal dependencies.",
    },
    ExternalProfile {
        id: "trellis2-cuda",
        accelerator: "NVIDIA CUDA",
        textured: true,
        input_mode: GenerationInputMode::Image,
        repository: "https://github.com/microsoft/TRELLIS.2",
        revision: "75fbf0183001ed9876c8dbb35de6b68552ee08bd",
        entry: "trellis2/pipelines/trellis2_image_to_3d.py",
        notice: "TRELLIS.2 CUDA uses gated DINOv3 and the official nvdiffrast baker; nvdiffrast v0.4.0 restricts use to non-commercial research or evaluation.",
    },
    ExternalProfile {
        id: "triposg-mlx",
        accelerator: "Apple MLX",
        textured: false,
        input_mode: GenerationInputMode::Image,
        repository: "https://github.com/cavi-ai/mlx-agent",
        revision: "c4c04b4cbbdf563fa3ae931f9a584bb7f0b240d5",
        entry: "src/mlx_agent/resources/ports/mlx-3d/triposg/pipeline.py",
        notice: "The MLX port carries HunyuanDiT terms. Review both the port and the scope of the TripoSG checkpoint before use.",
    },
    ExternalProfile {
        id: "triposg-cuda",
        accelerator: "NVIDIA CUDA",
        textured: false,
        input_mode: GenerationInputMode::Image,
        repository: "https://github.com/VAST-AI-Research/TripoSG",
        revision: "fc5c40990181e2a756c4e0b1c2f4d6b5202faf8c",
        entry: "triposg/pipelines/pipeline_triposg.py",
        notice: "The official FlashVDM path is not authorized by its community license in South Korea, the EU, or the UK, including non-commercial use. A separate grant is needed for covered use there.",
    },
    ExternalProfile {
        id: TRELLIS_TEXT_CUDA_ID,
        accelerator: "NVIDIA CUDA",
        textured: false,
        input_mode: GenerationInputMode::Text,
        repository: "https://github.com/microsoft/TRELLIS",
        revision: "193e0502b5cb3eb5997ec457d5340e95d09b158e",
        entry: "trellis/pipelines/trellis_text_to_3d.py",
        notice: "TRELLIS v1 Text Large exports geometry only. Review the CLIP and CUDA dependency terms in docs/trellis-text-cuda.md.",
    },
    ExternalProfile {
        id: TRELLIS_TEXT_MLX_ID,
        accelerator: "Apple MLX",
        textured: false,
        input_mode: GenerationInputMode::Text,
        repository: "https://github.com/amirhossein-razlighi/mlx_diffuser",
        revision: "a26b42aee4e31999dbb4429226b66d896d49e1d8",
        entry: "src/mlx_diffuser/models/trellis_slat.py",
        notice: "MeshHarbor's experimental native MLX port of TRELLIS v1 Text Large exports geometry only. See docs/trellis-text-mlx.md for setup and dependency terms.",
    },
];

pub(crate) const TRELLIS_CHECKPOINTS: &[&str] = &[
    "ckpts/ss_flow_img_dit_1_3B_64_bf16",
    "ckpts/shape_dec_next_dc_f16c32_fp16",
    "ckpts/slat_flow_img2shape_dit_1_3B_512_bf16",
    "ckpts/slat_flow_img2shape_dit_1_3B_1024_bf16",
    "ckpts/tex_dec_next_dc_f16c32_fp16",
    "ckpts/slat_flow_imgshape2tex_dit_1_3B_512_bf16",
    "ckpts/slat_flow_imgshape2tex_dit_1_3B_1024_bf16",
    "microsoft/TRELLIS-image-large/ckpts/ss_dec_conv3d_16l8_fp16",
];

pub(crate) const TRIPOSG_CUDA_FILES: &[&str] = &[
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

pub(crate) const TRELLIS_TEXT_MODEL_FILES: &[&str] = &[
    "model/pipeline.json",
    "model/ckpts/ss_flow_txt_dit_L_16l8_fp16.json",
    "model/ckpts/ss_flow_txt_dit_L_16l8_fp16.safetensors",
    "model/ckpts/slat_flow_txt_dit_L_64l8p2_fp16.json",
    "model/ckpts/slat_flow_txt_dit_L_64l8p2_fp16.safetensors",
    "decoders/ckpts/ss_dec_conv3d_16l8_fp16.json",
    "decoders/ckpts/ss_dec_conv3d_16l8_fp16.safetensors",
    "decoders/ckpts/slat_dec_mesh_swin8_B_64l8m256c_fp16.json",
    "decoders/ckpts/slat_dec_mesh_swin8_B_64l8m256c_fp16.safetensors",
    "clip/config.json",
    "clip/model.safetensors",
    "clip/merges.txt",
    "clip/special_tokens_map.json",
    "clip/tokenizer.json",
    "clip/tokenizer_config.json",
    "clip/vocab.json",
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

    pub fn host_supported(&self) -> bool {
        matches!(
            (
                self.id.ends_with("-mlx"),
                std::env::consts::OS,
                std::env::consts::ARCH,
            ),
            (true, "macos", "aarch64") | (false, "linux", "x86_64")
        )
    }

    fn legacy_root(&self, data_root: &Path) -> PathBuf {
        data_root.join("runtimes").join(self.id)
    }

    fn active_root(&self, data_root: &Path) -> PathBuf {
        let managed = self.managed_root(data_root);
        let owned = self.check_owner(data_root).unwrap_or(false);
        if owned && self.check_local_files(&managed).ready() {
            return managed;
        }
        let legacy = self.legacy_root(data_root);
        if !owned || self.check_local_files(&legacy).ready() {
            legacy
        } else {
            managed
        }
    }

    pub(crate) fn required_model_files(&self) -> Vec<String> {
        if matches!(self.id, TRELLIS_TEXT_CUDA_ID | TRELLIS_TEXT_MLX_ID) {
            TRELLIS_TEXT_MODEL_FILES
                .iter()
                .map(|file| (*file).into())
                .collect()
        } else if self.needs_dinov3() {
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

    pub(crate) fn needs_dinov3(&self) -> bool {
        matches!(self.id, "trellis2-mlx" | "trellis2-cuda")
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
        let flexicubes_file = match self.id {
            TRELLIS_TEXT_CUDA_ID => {
                Some("source/trellis/representations/mesh/flexicubes/flexicubes.py")
            }
            TRELLIS_TEXT_MLX_ID => Some("source/flexicubes/tables.py"),
            _ => None,
        };
        if let Some(file) = flexicubes_file {
            let flexicubes = root.join(file);
            if !is_nonempty_file(&flexicubes) {
                missing.push(format!(
                    "Missing local runtime file: {}",
                    flexicubes.display()
                ));
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
        self.host_supported() && self.check_local_files(&self.active_root(data_root)).ready()
    }

    pub fn status(&self, data_root: &Path, cuda_supported: bool) -> Value {
        let root = self.active_root(data_root);
        let managed = self.managed_root(data_root);
        let readiness = self.readiness(&root);
        let supported = self.host_supported() && (self.id.ends_with("-mlx") || cuda_supported);
        let managed_ready = root == managed && readiness.ready();
        let removable = self.removable_model_files(data_root);
        json!({
            "id": self.id,
            "generation": generation_profile(self.id),
            "accelerator": self.accelerator,
            "managed_install": true,
            "managed_ready": managed_ready,
            "host_supported": supported,
            "ready": readiness.ready(),
            "status": if readiness.ready() { readiness.status } else if supported { "installation_required" } else { "unsupported_host" },
            "validation_level": if managed_ready { "environment_preflight_and_required_paths" } else { "operator_activation_and_required_paths_only" },
            "runtime_root": if readiness.ready() { &root } else { &managed },
            "model_paths": if readiness.ready() && root != managed { vec![model_path(root.join("weights")), model_path(managed.join("weights"))] } else { vec![model_path(managed.join("weights"))] },
            "installable": supported,
            "weights_included": false,
            "downloads_enabled": supported && !managed_ready,
            "models_downloaded": managed_ready,
            "local_files": {
                "model_files_managed": removable.is_ok(),
                "remove_models_available": removable.unwrap_or(false),
            },
            "gated_model_access_required": self.needs_dinov3(),
            "source": { "repository": self.repository, "revision": self.revision },
            "license_notice": self.notice,
            "missing": if supported && !readiness.ready() { vec![] } else { readiness.missing },
        })
    }

    pub fn require_ready(&self, data_root: &Path) -> CoreResult<()> {
        let readiness = self.readiness(&self.active_root(data_root));
        if readiness.ready() {
            return Ok(());
        }
        let legacy = self.legacy_root(data_root);
        let details = if !self.host_supported() {
            readiness.missing.join(" ")
        } else if legacy.is_dir() {
            self.check_local_files(&legacy).missing.join(" ")
        } else {
            String::new()
        };
        let guidance = if details.is_empty() {
            " ".to_owned()
        } else {
            format!(" {details} ")
        };
        Err(CoreError::OperationUnavailable(format!(
            "{} is not ready.{guidance}Install it in Studio → Settings → Models. {}",
            self.id, self.notice,
        )))
    }

    pub fn generate(
        &self,
        data_root: &Path,
        input: ExternalInput<'_>,
        output: &Path,
        options: &GenerationOptions,
        execution: JobExecution<'_>,
    ) -> CoreResult<u128> {
        self.require_ready(data_root)?;
        let root = self.active_root(data_root).canonicalize()?;
        let (input_arg, stdin) = match (self.input_mode, input) {
            (GenerationInputMode::Image, ExternalInput::Image(path)) => {
                (path.canonicalize()?, None)
            }
            (GenerationInputMode::Text, ExternalInput::Text(prompt)) => {
                (PathBuf::from("-"), Some(prompt.as_bytes()))
            }
            _ => {
                return Err(CoreError::InvalidInput(
                    "Input does not match the external generation backend.".into(),
                ));
            }
        };
        let work = output
            .parent()
            .expect("generation output always has a work directory")
            .canonicalize()?;
        let output = work.join(output.file_name().expect("generation output has a name"));
        let adapter = work.join("external-generation.py");
        let adapter_source: &[u8] = match self.id {
            TRELLIS_TEXT_CUDA_ID => include_bytes!("trellis_text_generation.py"),
            TRELLIS_TEXT_MLX_ID => {
                fs::write(
                    work.join("mesh_decoder.py"),
                    include_bytes!("trellis_text_mlx/mesh_decoder.py"),
                )?;
                fs::write(
                    work.join("surface.py"),
                    include_bytes!("trellis_text_mlx/surface.py"),
                )?;
                include_bytes!("trellis_text_mlx/worker.py")
            }
            _ => include_bytes!("external_generation.py"),
        };
        fs::write(&adapter, adapter_source)?;
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
            .arg(input_arg)
            .arg(output)
            .arg(options.seed.to_string());
        if stdin.is_none() {
            command
                .arg(options.resolution.to_string())
                .arg(options.texture_size.to_string());
        }
        command.arg(options.target_faces.to_string());
        if self.id.ends_with("-cuda") {
            for key in ["CUDA_HOME", "CUDA_VISIBLE_DEVICES"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
        }
        let started = Instant::now();
        let status = run_generation_command_with_input(&mut command, execution, stdin)?;
        if !status.success() {
            return Err(CoreError::OperationUnavailable(format!(
                "{} worker failed ({status}). {}",
                self.id,
                execution.output_hint()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trellis_text_readiness_requires_clip_and_mesh_decoder_without_dinov3() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-trellis-text-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let profile = ExternalProfile::find(TRELLIS_TEXT_CUDA_ID).unwrap();
        assert_eq!(
            profile.check_local_files(&root).status,
            "manual_activation_required"
        );
        fs::write(
            root.join("activate.json"),
            json!({"source_revision": profile.revision, "authorized_local_use": true}).to_string(),
        )
        .unwrap();
        let mut files = profile
            .required_model_files()
            .into_iter()
            .map(|file| root.join("weights").join(file))
            .collect::<Vec<_>>();
        files.extend([
            root.join(".venv/bin/python"),
            root.join("source").join(profile.entry),
            root.join("source/trellis/representations/mesh/flexicubes/flexicubes.py"),
        ]);
        for file in files {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "fixture").unwrap();
        }
        assert!(profile.check_local_files(&root).ready());
        assert!(!profile.needs_dinov3());
        fs::remove_file(root.join("weights/clip/model.safetensors")).unwrap();
        fs::remove_file(
            root.join("weights/decoders/ckpts/slat_dec_mesh_swin8_B_64l8m256c_fp16.safetensors"),
        )
        .unwrap();
        let readiness = profile.check_local_files(&root);
        assert_eq!(readiness.status, "manual_files_required");
        assert_eq!(readiness.missing.len(), 2);
        assert!(
            readiness
                .missing
                .iter()
                .any(|message| message.ends_with("clip/model.safetensors"))
        );
        assert!(
            readiness.missing.iter().any(
                |message| message.ends_with("slat_dec_mesh_swin8_B_64l8m256c_fp16.safetensors")
            )
        );
        fs::remove_dir_all(root).unwrap();
    }
}
