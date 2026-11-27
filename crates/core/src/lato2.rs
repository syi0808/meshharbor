use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use meshharbor_atlas_baker::{
    AtlasBakeOptions, bake_normal_ray_projected_mesh, encode_projected_mesh_glb,
    extract_texture_rebake_source,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    CoreError, CoreResult,
    external_generation::ExternalProfile,
    glb::encode_triangle_geometry_glb,
    job_execution::JobExecution,
    operation::map_atlas_bake_error,
    retopology::{RemeshRequest, RemeshResponse, validate_remesh_response},
};

pub const LATO2_OPERATION: &str = "mesh.retopology_lato2";
pub const LATO2_IMAGE_BACKEND: &str = "lato2-cuda";
pub const LATO2_MLX_BACKEND: &str = "lato2-mlx";
pub(crate) const BACKENDS: [&str; 2] = [LATO2_MLX_BACKEND, LATO2_IMAGE_BACKEND];
pub(crate) const SOURCE_REPOSITORY: &str = "https://github.com/LoHhhha/LATO.2";
pub(crate) const SOURCE_REVISION: &str = "650b48fb1257c83ec1358f0c9f1d95d5cb9c7468";
pub(crate) const CHECKPOINTS: [&str; 6] = [
    "vflow.pt",
    "vvae.pt",
    "offset_head.pt",
    "tflow.pt",
    "tvae.pt",
    "voxel_encoder.pt",
];
pub(crate) const TRELLIS_FILES: [&str; 4] = [
    "ckpts/ss_flow_img_dit_L_16l8_fp16.json",
    "ckpts/ss_flow_img_dit_L_16l8_fp16.safetensors",
    "ckpts/ss_dec_conv3d_16l8_fp16.json",
    "ckpts/ss_dec_conv3d_16l8_fp16.safetensors",
];
pub(crate) const BIREFNET_FILES: [&str; 4] = [
    "config.json",
    "model.safetensors",
    "BiRefNet_config.py",
    "birefnet.py",
];
pub(crate) const DINO_WEIGHT: &str = "dinov2_vitl14_reg4_pretrain.pth";
pub(crate) const DINO_SOURCE: &str = "facebookresearch_dinov2_main";
pub(crate) const DEFAULT_TARGET_VERTICES: u32 = 2000;
pub(crate) const MIN_TARGET_VERTICES: u32 = 200;
pub(crate) const MAX_TARGET_VERTICES: u32 = 5000;
const DEFAULT_TARGET_TRIANGLES: usize = 4000;
const MAX_GLB_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OBJ_BYTES: u64 = 4 * 1024 * 1024;
const SEED: u32 = 42;
const RAY_DISTANCE_RATIO: f32 = 0.03;

const INFERENCE_ENTRY: &str = r#"
import runpy
import sys
import types
from importlib.util import find_spec
import torch

def no_download(*args, **kwargs):
    raise RuntimeError("LATO.2 model files must be installed locally before inference")

original_load = torch.load
def load_weights(*args, **kwargs):
    kwargs["weights_only"] = True
    return original_load(*args, **kwargs)

torch.load = load_weights
torch.hub.download_url_to_file = no_download
if sys.argv[1].endswith("/e2e_inference.py"):
    spec = find_spec("o_voxel")
    if spec is None:
        raise RuntimeError("LATO.2 Mesh to Mesh needs the pinned o_voxel converter")
    converter = types.ModuleType("o_voxel")
    converter.__path__ = spec.submodule_search_locations
    sys.modules["o_voxel"] = converter
sys.argv = sys.argv[1:]
runpy.run_path(sys.argv[0], run_name="__main__")
"#;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Lato2Options {
    backend_id: String,
    target_triangles: usize,
    atlas_resolution: u32,
}

impl Default for Lato2Options {
    fn default() -> Self {
        Self {
            backend_id: "auto".into(),
            target_triangles: DEFAULT_TARGET_TRIANGLES,
            atlas_resolution: 1024,
        }
    }
}

impl Lato2Options {
    pub(crate) fn parse(parameters: serde_json::Value) -> CoreResult<Self> {
        let options: Option<Self> = serde_json::from_value(parameters).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid {LATO2_OPERATION} parameters: {error}"))
        })?;
        Ok(options.unwrap_or_default())
    }

    fn validate(&self) -> CoreResult<()> {
        if self.target_triangles < 4 {
            return Err(CoreError::InvalidInput(
                "target_triangles must be at least 4.".into(),
            ));
        }
        if !matches!(self.atlas_resolution, 512 | 1024 | 2048) {
            return Err(CoreError::InvalidInput(
                "atlas_resolution must be 512, 1024, or 2048.".into(),
            ));
        }
        Ok(())
    }

    fn conditioning_vertices(&self) -> u32 {
        (self.target_triangles.div_ceil(2) + 2)
            .clamp(MIN_TARGET_VERTICES as usize, MAX_TARGET_VERTICES as usize) as u32
    }
}

#[derive(Serialize)]
pub(crate) struct Lato2Report {
    algorithm: &'static str,
    backend_id: &'static str,
    target_triangles: usize,
    conditioning_vertices: u32,
    seed: u32,
    input_triangles: usize,
    output_triangles: usize,
    retopology_vertices: usize,
    output_vertices: usize,
    atlas_resolution: u32,
    atlas_width: u32,
    atlas_height: u32,
    rebaked_maps: Vec<String>,
    projected_texels: u64,
    occupied_texels: u64,
    projection_coverage: f32,
    ambiguous_texels: u64,
    infilled_texels: u64,
    unfilled_texels: u64,
    projection_distance_limit: f32,
    maximum_projection_distance: f32,
    topology: &'static str,
    warnings: Vec<String>,
}

pub(crate) struct Lato2Runtime {
    id: &'static str,
    checkout: PathBuf,
    python: PathBuf,
    weights: PathBuf,
    home: Option<PathBuf>,
}

pub fn is_backend(id: &str) -> bool {
    BACKENDS.contains(&id)
}

impl Lato2Runtime {
    fn operator_cuda() -> Option<Self> {
        let checkout = configured_path("MESHHARBOR_LATO2_DIR")?;
        let python = configured_path("MESHHARBOR_LATO2_PYTHON")?;
        if !python.is_file() {
            return None;
        }
        let weights = checkout.join("ckpt");
        if !checkout.join("scripts/e2e_inference.py").is_file()
            || !weights
                .join("dinov2")
                .join(DINO_SOURCE)
                .join("hubconf.py")
                .is_file()
            || CHECKPOINTS.iter().any(|file| !weights.join(file).is_file())
        {
            return None;
        }
        Some(Self {
            id: LATO2_IMAGE_BACKEND,
            checkout,
            python,
            weights,
            home: None,
        })
    }

    pub(crate) fn discover(root: &Path, id: &str) -> Option<Self> {
        let profile = ExternalProfile::find(id).filter(|profile| is_backend(profile.id))?;
        if profile.is_ready(root) {
            let runtime = profile.active_root(root);
            return Some(Self {
                id: profile.id,
                checkout: runtime.join("source"),
                weights: runtime.join("weights"),
                python: runtime.join(".venv/bin/python"),
                home: Some(runtime.join("home")),
            });
        }
        if id == LATO2_IMAGE_BACKEND {
            Self::operator_cuda()
        } else {
            None
        }
    }

    fn missing_image_files(&self) -> Vec<String> {
        let paths = std::iter::once(self.checkout.join("scripts/image_e2e_inference.py"))
            .chain(std::iter::once(
                self.weights.join("dinov2/checkpoints").join(DINO_WEIGHT),
            ))
            .chain(
                TRELLIS_FILES
                    .iter()
                    .map(|file| self.weights.join("trellis").join(file)),
            )
            .chain(
                BIREFNET_FILES
                    .iter()
                    .map(|file| self.weights.join("rembg/BiRefNet").join(file)),
            );
        paths
            .filter(|file| !file.is_file())
            .map(|file| file.display().to_string())
            .collect()
    }

    pub(crate) fn image_ready(root: &Path, id: &str) -> bool {
        Self::discover(root, id).is_some_and(|runtime| runtime.missing_image_files().is_empty())
    }

    pub(crate) fn require_image(root: &Path, id: &str) -> CoreResult<Self> {
        if let Some(runtime) = Self::discover(root, id) {
            let missing = runtime.missing_image_files();
            if missing.is_empty() {
                return Ok(runtime);
            }
            return Err(CoreError::OperationUnavailable(format!(
                "LATO.2 Image to 3D is missing: {}",
                missing.join("; ")
            )));
        }
        Err(CoreError::OperationUnavailable(format!(
            "Install {id} under Studio → Settings → Models."
        )))
    }

    pub(crate) fn mesh_ready(root: &Path) -> bool {
        BACKENDS.iter().any(|id| Self::discover(root, id).is_some())
    }

    pub(crate) fn require_mesh(root: &Path, options: &Lato2Options) -> CoreResult<Self> {
        options.validate()?;
        if options.backend_id == "auto" {
            if let Some(runtime) = BACKENDS.iter().find_map(|id| Self::discover(root, id)) {
                return Ok(runtime);
            }
        } else if !is_backend(&options.backend_id) {
            return Err(CoreError::InvalidInput(
                "LATO.2 backend_id must be auto, lato2-cuda, or lato2-mlx.".into(),
            ));
        } else if let Some(runtime) = Self::discover(root, &options.backend_id) {
            return Ok(runtime);
        }
        Err(CoreError::OperationUnavailable(format!(
            "The requested LATO.2 backend ({}) is not ready. Install a matching backend under Studio → Settings → Models.",
            options.backend_id
        )))
    }

    fn inference_command(&self, work: &Path, script: &str) -> CoreResult<Command> {
        let mut inference = Command::new(&self.python);
        if let Some(home) = &self.home {
            fs::create_dir_all(home)?;
            inference
                .env_clear()
                .env(
                    "PATH",
                    std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
                )
                .env("HOME", home)
                .env("HF_HOME", home.join("huggingface"))
                .env("HF_HUB_DISABLE_IMPLICIT_TOKEN", "1");
            if self.id == LATO2_IMAGE_BACKEND {
                for key in ["CUDA_HOME", "CUDA_VISIBLE_DEVICES"] {
                    if let Some(value) = std::env::var_os(key) {
                        inference.env(key, value);
                    }
                }
                inference
                    .env("SPARSE_ATTN_BACKEND", "xformers")
                    .env("SPARSE_BACKEND", "spconv");
            }
        }
        inference
            .current_dir(&self.checkout)
            .env("XDG_RUNTIME_DIR", work)
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env_remove("MESHHARBOR_API_TOKEN")
            .env_remove("HF_TOKEN")
            .env_remove("HUGGING_FACE_HUB_TOKEN")
            .args(["-I", "-B", "-u"]);
        if self.id == LATO2_MLX_BACKEND {
            let worker = work.join("lato2_mlx_worker.py");
            fs::write(&worker, include_bytes!("lato2_mlx/worker.py"))?;
            fs::write(
                work.join("mlx_kernels.py"),
                include_bytes!("lato2_mlx/mlx_kernels.py"),
            )?;
            fs::write(
                work.join("mesh_conditioning.py"),
                include_bytes!("lato2_mlx/mesh_conditioning.py"),
            )?;
            inference.arg(worker);
        } else {
            inference.args(["-c", INFERENCE_ENTRY]);
        }
        inference
            .arg(self.checkout.join("scripts").join(script))
            .arg("--dino_hub_dir")
            .arg(self.weights.join("dinov2"));
        if self.home.is_some() {
            for checkpoint in CHECKPOINTS {
                let stem = checkpoint.trim_end_matches(".pt");
                inference
                    .arg(format!("--{stem}_ckpt"))
                    .arg(self.weights.join("lato2").join(checkpoint));
            }
            if script == "image_e2e_inference.py" {
                inference
                    .arg("--trellis_dir")
                    .arg(self.weights.join("trellis"))
                    .arg("--rembg_dir")
                    .arg(self.weights.join("rembg"));
            }
        }
        Ok(inference)
    }

    pub(crate) fn generate_image(
        &self,
        image: &Path,
        output: &Path,
        target_vertices: u32,
        seed: u64,
        execution: JobExecution<'_>,
    ) -> CoreResult<u128> {
        let now = Instant::now();
        let work = WorkDir::create()?;
        let input = work.0.join("input");
        let predictions = work.0.join("predictions");
        fs::create_dir(&input)?;
        fs::create_dir(&predictions)?;
        let extension = match image.extension().and_then(|value| value.to_str()) {
            Some("png") => "png",
            Some("jpg" | "jpeg") => "jpg",
            _ => {
                return Err(CoreError::InvalidInput(
                    "LATO.2 expects a PNG or JPEG reference.".into(),
                ));
            }
        };
        fs::copy(image, input.join("source").with_extension(extension))?;
        execution.stage("lato2_image_inference")?;
        let mut inference = self.inference_command(&work.0, "image_e2e_inference.py")?;
        inference
            .arg("--image_dir")
            .arg(&input)
            .arg("--out_dir")
            .arg(&predictions)
            .args(["--batch_size", "1", "--seed"])
            .arg(seed.to_string())
            .arg("--vert_num")
            .arg(target_vertices.to_string());
        execution
            .with_timeout(Duration::from_secs(if self.id == LATO2_MLX_BACKEND {
                1200
            } else {
                600
            }))
            .visible(inference, "LATO.2 image inference failed")?;

        execution.stage("validate_lato2_topology")?;
        let mut geometry = load_prediction(&predictions)?;
        orient_closed_mesh(&mut geometry)?;
        let request = RemeshRequest {
            protocol_version: 1,
            positions: geometry.positions.clone(),
            indices: Vec::new(),
            options: Default::default(),
        };
        validate_remesh_response(&geometry, &request)?;
        execution.checkpoint()?;
        fs::write(
            output,
            encode_triangle_geometry_glb(
                &geometry.positions,
                &geometry.indices,
                "MeshHarbor LATO.2",
            )?,
        )?;
        Ok(now.elapsed().as_millis())
    }

    pub(crate) fn retopologize(
        &self,
        bytes: &[u8],
        options: Lato2Options,
        execution: JobExecution<'_>,
    ) -> CoreResult<(Vec<u8>, Lato2Report)> {
        validate_source_size(bytes.len() as u64)?;
        execution.stage("validate_lato2_source")?;
        let (mut source, has_mr) =
            extract_texture_rebake_source(bytes).map_err(map_atlas_bake_error)?;
        compact_positions(&mut source.positions, &mut source.indices);
        let frame = Frame::from_positions(&source.positions)?;
        let input_triangles = source.indices.len() / 3;
        let conditioning_vertices = options.conditioning_vertices();
        let work = WorkDir::create()?;
        let input = work.0.join("input");
        let predictions = work.0.join("predictions");
        fs::create_dir(&input)?;
        fs::create_dir(&predictions)?;
        write_source_obj(
            &input.join("source.obj"),
            &source.positions,
            &source.indices,
        )?;

        execution.stage("lato2_inference")?;
        let mut inference = self.inference_command(&work.0, "e2e_inference.py")?;
        inference
            .arg("--mesh_dir")
            .arg(&input)
            .arg("--out_dir")
            .arg(&predictions)
            .args(["--num_workers", "0", "--batch_size", "1", "--seed"])
            .arg(SEED.to_string())
            .arg("--vert_num")
            .arg(conditioning_vertices.to_string());
        execution
            .with_timeout(Duration::from_secs(if self.id == LATO2_MLX_BACKEND {
                900
            } else {
                300
            }))
            .visible(inference, "LATO.2 inference failed")?;

        execution.stage("validate_lato2_topology")?;
        let mut geometry = load_prediction(&predictions)?;
        let output_triangles = geometry.indices.len() / 3;
        orient_closed_mesh(&mut geometry)?;
        let request = RemeshRequest {
            protocol_version: 1,
            positions: source
                .positions
                .iter()
                .map(|position| frame.normalize(*position))
                .collect(),
            indices: source.indices,
            options: Default::default(),
        };
        validate_remesh_response(&geometry, &request)?;

        execution.stage("lato2_texture_rebake")?;
        let normals = smooth_normals(&geometry);
        let positions = geometry
            .positions
            .iter()
            .map(|position| frame.restore(*position))
            .collect::<Vec<_>>();
        let baked = bake_normal_ray_projected_mesh(
            bytes,
            &positions,
            &normals,
            &geometry.indices,
            AtlasBakeOptions {
                resolution: options.atlas_resolution,
                padding: options.atlas_resolution / 128,
            },
            RAY_DISTANCE_RATIO,
        )
        .map_err(map_atlas_bake_error)?;
        execution.stage("validate_lato2_output")?;
        let output = encode_projected_mesh_glb(&baked).map_err(map_atlas_bake_error)?;
        let (result, output_has_mr) =
            extract_texture_rebake_source(&output).map_err(map_atlas_bake_error)?;
        if result.indices.len() / 3 != output_triangles || output_has_mr != has_mr {
            return Err(CoreError::OperationUnavailable(
                "LATO.2 export changed the triangle count or lost a texture map.".into(),
            ));
        }
        let report = baked.report;
        let infilled = report.projection_infilled_texels.unwrap_or(0);
        let unfilled = report
            .projection_attempted_texels
            .saturating_sub(report.projection_covered_texels + infilled);
        let mut warnings = report.warnings;
        if unfilled != 0 {
            warnings
                .push("Some atlas texels could not be locally filled and may show gaps.".into());
        }
        Ok((
            output,
            Lato2Report {
                algorithm: "lato2+xatlas-normal-rays",
                backend_id: self.id,
                target_triangles: options.target_triangles,
                conditioning_vertices,
                seed: SEED,
                input_triangles,
                output_triangles,
                retopology_vertices: geometry.positions.len(),
                output_vertices: result.positions.len(),
                atlas_resolution: options.atlas_resolution,
                atlas_width: report.atlas_width,
                atlas_height: report.atlas_height,
                rebaked_maps: report.rebaked_maps,
                projected_texels: report.projection_covered_texels,
                occupied_texels: report.projection_attempted_texels,
                projection_coverage: report.projection_coverage,
                ambiguous_texels: report.ambiguous_texels,
                infilled_texels: infilled,
                unfilled_texels: unfilled,
                projection_distance_limit: report.projection_distance_limit,
                maximum_projection_distance: report.maximum_projection_distance,
                topology: "closed_oriented_manifold",
                warnings,
            },
        ))
    }
}

pub(crate) fn validate_source_size(byte_size: u64) -> CoreResult<()> {
    if byte_size > MAX_GLB_BYTES {
        return Err(CoreError::InvalidInput(
            "LATO.2 supports source GLBs up to 64 MiB.".into(),
        ));
    }
    Ok(())
}

fn configured_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

struct WorkDir(PathBuf);

impl WorkDir {
    fn create() -> CoreResult<Self> {
        let path = std::env::temp_dir().join(format!("meshharbor-lato2-{}", Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Frame {
    center: [f64; 3],
    extent: f64,
}

impl Frame {
    fn from_positions(positions: &[[f32; 3]]) -> CoreResult<Self> {
        let mut lower = [f64::INFINITY; 3];
        let mut upper = [f64::NEG_INFINITY; 3];
        for position in positions {
            for (axis, value) in position.iter().copied().map(f64::from).enumerate() {
                lower[axis] = lower[axis].min(value);
                upper[axis] = upper[axis].max(value);
            }
        }
        let center = std::array::from_fn(|axis| (lower[axis] + upper[axis]) * 0.5);
        let extent = (0..3)
            .map(|axis| upper[axis] - lower[axis])
            .fold(0.0_f64, f64::max);
        if !extent.is_finite() || extent < 1e-7 {
            return Err(CoreError::InvalidInput(
                "LATO.2 requires a non-degenerate source extent of at least 1e-7.".into(),
            ));
        }
        Ok(Self { center, extent })
    }

    fn normalize(&self, position: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|axis| {
            ((f64::from(position[axis]) - self.center[axis]) / self.extent) as f32
        })
    }

    fn restore(&self, position: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|axis| {
            (f64::from(position[axis]) * self.extent + self.center[axis]) as f32
        })
    }
}

fn smooth_normals(geometry: &RemeshResponse) -> Vec<[f32; 3]> {
    let mut averages = vec![[0.0_f64; 3]; geometry.positions.len()];
    let mut fallback = averages.clone();
    for triangle in geometry.indices.chunks_exact(3) {
        let first = geometry.positions[triangle[0] as usize].map(f64::from);
        let second = geometry.positions[triangle[1] as usize].map(f64::from);
        let third = geometry.positions[triangle[2] as usize].map(f64::from);
        let along: [f64; 3] = std::array::from_fn(|axis| second[axis] - first[axis]);
        let across: [f64; 3] = std::array::from_fn(|axis| third[axis] - first[axis]);
        let face: [f64; 3] = std::array::from_fn(|axis| {
            along[(axis + 1) % 3] * across[(axis + 2) % 3]
                - along[(axis + 2) % 3] * across[(axis + 1) % 3]
        });
        for &index in triangle {
            let index = index as usize;
            for (sum, weight) in averages[index].iter_mut().zip(face) {
                *sum += weight;
            }
            fallback[index] = face;
        }
    }
    averages
        .into_iter()
        .zip(fallback)
        .map(|(mut normal, fallback)| {
            let length =
                |normal: [f64; 3]| normal.iter().map(|axis| axis * axis).sum::<f64>().sqrt();
            if length(normal) <= 1e-14 {
                normal = fallback;
            }
            let scale = length(normal);
            normal.map(|axis| (axis / scale) as f32)
        })
        .collect()
}

fn write_source_obj(path: &Path, positions: &[[f32; 3]], indices: &[u32]) -> CoreResult<()> {
    let mut file = BufWriter::new(File::create(path)?);
    for [x, y, z] in positions {
        writeln!(file, "v {x} {y} {z}")?;
    }
    for face in indices.chunks_exact(3) {
        writeln!(file, "f {} {} {}", face[0] + 1, face[1] + 1, face[2] + 1)?;
    }
    file.flush()?;
    Ok(())
}

fn compact_positions(positions: &mut Vec<[f32; 3]>, indices: &mut [u32]) {
    let mut remap = vec![None; positions.len()];
    for &index in indices.iter() {
        remap[index as usize] = Some(0);
    }
    let mut retained = 0;
    let mut entries = remap.iter_mut();
    positions.retain(|_| {
        if let Some(index) = entries.next().unwrap() {
            *index = retained;
            retained += 1;
            true
        } else {
            false
        }
    });
    for index in indices {
        *index = remap[*index as usize].unwrap();
    }
}

fn read_bounded(path: &Path, max: u64) -> CoreResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| bad_prediction(format!("could not read the worker output: {error}")))?;
    if !metadata.is_file() {
        return Err(bad_prediction("worker did not produce a regular file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > max {
        return Err(bad_prediction(
            "worker output is empty or exceeds its size limit",
        ));
    }
    Ok(bytes)
}

fn bad_prediction(reason: impl std::fmt::Display) -> CoreError {
    CoreError::OperationUnavailable(format!("LATO.2 output rejected: {reason}."))
}

fn load_prediction(predictions: &Path) -> CoreResult<RemeshResponse> {
    let obj = read_bounded(&predictions.join("source_pred.obj"), MAX_OBJ_BYTES)?;
    parse_prediction(std::str::from_utf8(&obj).map_err(|_| bad_prediction("invalid OBJ text"))?)
}

fn parse_prediction(obj: &str) -> CoreResult<RemeshResponse> {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for line in obj.lines() {
        let mut tokens = line.split_ascii_whitespace();
        match tokens.next() {
            None => (),
            Some(comment) if comment.starts_with('#') => (),
            Some("v") => {
                let mut position = [0.0_f32; 3];
                for value in &mut position {
                    *value = tokens
                        .next()
                        .and_then(|token| token.parse().ok())
                        .filter(|value: &f32| value.is_finite() && value.abs() <= 0.55)
                        .ok_or_else(|| bad_prediction("invalid normalized OBJ vertex"))?;
                }
                if tokens.next().is_some() {
                    return Err(bad_prediction("invalid OBJ vertex arity"));
                }
                positions.push(position);
            }
            Some("f") => {
                for _ in 0..3 {
                    let index = tokens
                        .next()
                        .and_then(|token| token.parse::<u32>().ok())
                        .and_then(|index| index.checked_sub(1))
                        .ok_or_else(|| bad_prediction("invalid OBJ face index"))?;
                    indices.push(index);
                }
                if tokens.next().is_some() {
                    return Err(bad_prediction("only triangle faces are accepted"));
                }
            }
            _ => return Err(bad_prediction("unexpected OBJ record")),
        }
    }
    if positions.is_empty()
        || indices.len() < 12
        || indices
            .iter()
            .any(|index| *index as usize >= positions.len())
    {
        return Err(bad_prediction("empty mesh or out-of-range face index"));
    }
    Ok(RemeshResponse { positions, indices })
}

fn orient_closed_mesh(mesh: &mut RemeshResponse) -> CoreResult<()> {
    let mut edges: HashMap<_, (usize, bool, bool)> = HashMap::new();
    let mut neighbors = vec![Vec::new(); mesh.indices.len() / 3];
    for (face, triangle) in mesh.indices.chunks_exact(3).enumerate() {
        for (from, to) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            if from == to {
                return Err(bad_prediction("degenerate face"));
            }
            let order = from < to;
            let (other, direction, paired) = edges
                .entry((from.min(to), from.max(to)))
                .or_insert((face, order, false));
            if *other != face {
                if *paired {
                    return Err(bad_prediction("non-manifold edge"));
                }
                *paired = true;
                neighbors[face].push((*other, order == *direction));
                neighbors[*other].push((face, order == *direction));
            }
        }
    }
    if edges.values().any(|(_, _, paired)| !paired) {
        return Err(bad_prediction("open mesh"));
    }
    let mut flip = vec![None; neighbors.len()];
    for start in 0..neighbors.len() {
        if flip[start].is_some() {
            continue;
        }
        flip[start] = Some(false);
        let mut component = vec![start];
        let mut cursor = 0;
        let mut volume = 0.0;
        while let Some(&face) = component.get(cursor) {
            let reversed = flip[face].unwrap();
            let triangle = &mesh.indices[face * 3..face * 3 + 3];
            let [first, second, third] = [triangle[0], triangle[1], triangle[2]]
                .map(|index| mesh.positions[index as usize].map(f64::from));
            let signed = (0..3)
                .map(|axis| {
                    first[axis]
                        * (second[(axis + 1) % 3] * third[(axis + 2) % 3]
                            - second[(axis + 2) % 3] * third[(axis + 1) % 3])
                })
                .sum::<f64>();
            volume += if reversed { -signed } else { signed };
            for &(neighbor, invert) in &neighbors[face] {
                let required = reversed ^ invert;
                match flip[neighbor] {
                    Some(current) if current != required => {
                        return Err(bad_prediction("non-orientable surface"));
                    }
                    Some(_) => (),
                    None => {
                        flip[neighbor] = Some(required);
                        component.push(neighbor);
                    }
                }
            }
            cursor += 1;
        }
        if volume.abs() <= 1e-12 {
            return Err(bad_prediction("component has negligible signed volume"));
        }
        for face in component {
            if flip[face].unwrap() ^ (volume < 0.0) {
                mesh.indices.swap(face * 3 + 1, face * 3 + 2);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TETRA: &str =
        "v 0 0 0\nv 0.5 0 0\nv 0 0.5 0\nv 0 0 0.5\nf 1 2 3\nf 1 2 4\nf 1 4 3\nf 2 3 4\n";

    #[test]
    fn repairs_winding_and_rejects_invalid_topology() {
        let mut mesh = parse_prediction(TETRA).unwrap();
        orient_closed_mesh(&mut mesh).unwrap();
        assert_eq!(&mesh.indices[..3], &[0, 2, 1]);
        let request = RemeshRequest {
            protocol_version: 1,
            positions: mesh.positions.clone(),
            indices: mesh.indices.clone(),
            options: Default::default(),
        };
        validate_remesh_response(&mesh, &request).unwrap();
        let mut nonmanifold = parse_prediction(&format!("{TETRA}f 1 2 3\n")).unwrap();
        assert!(orient_closed_mesh(&mut nonmanifold).is_err());
        let mut open = parse_prediction(TETRA).unwrap();
        open.indices.truncate(9);
        assert!(orient_closed_mesh(&mut open).is_err());
    }

    #[test]
    fn ignores_unreferenced_source_vertices_when_normalizing() {
        let mut positions = vec![
            [500.0; 3],
            [2.0, 3.0, 4.0],
            [4.0, 3.0, 4.0],
            [2.0, 5.0, 4.0],
        ];
        let mut indices = [1, 2, 3];
        compact_positions(&mut positions, &mut indices);
        assert_eq!(indices, [0, 1, 2]);
        let frame = Frame::from_positions(&positions).unwrap();
        assert_eq!(frame.center, [3.0, 4.0, 4.0]);
        assert_eq!(frame.extent, 2.0);
    }

    #[test]
    fn rejects_invalid_obj_and_options() {
        assert!(parse_prediction(&TETRA.replace("0.5 0 0", "NaN 0 0")).is_err());
        assert!(parse_prediction(&TETRA.replace("f 1 2 4", "f 1 2 8")).is_err());
        assert!(parse_prediction(&TETRA.replace("f 1 2 4", "f 1 2 3 4")).is_err());
        for params in [r#"{"target_triangles":3}"#, r#"{"atlas_resolution":4096}"#] {
            assert!(
                serde_json::from_str::<Lato2Options>(params)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        for obsolete in [
            r#"{"script":"ignored"}"#,
            r#"{"max_triangles":6000}"#,
            r#"{"target_vertices":1200}"#,
        ] {
            assert!(serde_json::from_str::<Lato2Options>(obsolete).is_err());
        }
    }

    #[test]
    fn triangle_target_only_conditions_density() {
        for (target_triangles, conditioning_vertices) in
            [(4, 200), (3008, 1506), (3009, 1507), (usize::MAX, 5000)]
        {
            let options = Lato2Options {
                target_triangles,
                ..Lato2Options::default()
            };
            assert!(options.validate().is_ok());
            assert_eq!(options.conditioning_vertices(), conditioning_vertices);
        }
        let large_obj = format!("{TETRA}{}", "f 1 2 3\n".repeat(20_000));
        assert_eq!(
            parse_prediction(&large_obj).unwrap().indices.len() / 3,
            20_004
        );
    }
}
