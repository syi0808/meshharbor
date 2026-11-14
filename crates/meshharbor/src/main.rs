use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::http::Method;
use axum::{
    Json, Router,
    body::Bytes,
    extract::DefaultBodyLimit,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::{Parser, Subcommand};
use meshharbor_core::{CoreError, GenerationOptions, OperationCore};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{fs, net::TcpListener, sync::Semaphore, task};
use tracing_subscriber::EnvFilter;

include!(concat!(env!("OUT_DIR"), "/studio_assets.rs"));

mod data_dir_lock;

/// Prevents fork/exec lock tests from inheriting another test's live advisory-lock descriptor.
#[cfg(test)]
pub(crate) static DATA_DIR_LOCK_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Parser)]
#[command(
    name = "meshharbor",
    version,
    about = "Local-first 3D asset processing core"
)]
struct Cli {
    #[arg(long, default_value_os_t = default_data_dir())]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the localhost REST API.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7432")]
        listen: SocketAddr,
    },
    /// Start the stdio MCP server.
    Mcp,
    /// Import a PNG, JPEG, or GLB asset into the local store.
    Import { file: PathBuf },
    /// Inspect a supported asset file without importing it.
    Inspect { file: PathBuf },
    /// List assets currently in the local store.
    Assets {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Simplify a previously imported static indexed GLB mesh.
    Simplify {
        asset_id: String,
        #[arg(long)]
        target_triangles: u64,
        #[arg(long, default_value_t = 0.02)]
        max_error: f32,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Run a supported local mesh operation. For retopology, --output writes the triangle GLB preview and --quad-output preserves the editable OBJ quads.
    /// Part separation limits: 64 MiB GLB, 4 MiB JSON, 1,000,000 vertices, 3,000,000 indices, and 256 output parts.
    ProcessMesh {
        asset_id: String,
        #[arg(long, value_parser = ["mesh.cleanup", "mesh.separate_parts", "uv.unwrap", "render.optimize", "mesh.compress", "mesh.retopology"])]
        operation: String,
        /// Requested approximate quad count for mesh.retopology (4 to 50,000).
        #[arg(long)]
        target_quads: Option<u32>,
        /// Optional path for a derived GLB or the triangulated retopology preview.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Optional path for the original editable quad OBJ from mesh.retopology.
        #[arg(long)]
        quad_output: Option<PathBuf>,
    },
    /// Generate a textured GLB from a previously imported image using a ready local Pixal3D backend.
    Generate {
        asset_id: String,
        #[arg(long, default_value = "auto", value_parser = ["auto", "pixal3d-mlx", "pixal3d-cuda"])]
        backend_id: String,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long, default_value_t = 512)]
        resolution: u16,
        #[arg(long, default_value_t = 1024)]
        texture_size: u16,
        #[arg(long, default_value_t = 100_000)]
        target_faces: u32,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Show implemented operations and detected runtime capabilities.
    Doctor,
    /// Inspect or request installation of a local mesh backend.
    Backend {
        #[command(subcommand)]
        command: BackendCommand,
    },
    /// Reconcile background jobs left unfinished by an earlier exit.
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
    },
    /// Manage the optional local Pixal3D MLX runtime.
    Runtime {
        #[command(subcommand)]
        command: RuntimeCommand,
    },
}

#[derive(Debug, Subcommand)]
enum BackendCommand {
    /// Show QuadriFlow availability and the next supported action.
    Status,
    /// Request the guarded installation for the named backend.
    Install {
        #[arg(value_parser = ["quadriflow"])]
        backend_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum JobsCommand {
    /// Mark unfinished jobs as failed after stopping every MeshHarbor process using this data directory.
    RecoverInterrupted {
        #[arg(
            long,
            required = true,
            action = clap::ArgAction::SetTrue,
            help = "Acknowledge that no MeshHarbor serve or mcp process is using this data directory."
        )]
        confirm_no_running_meshharbor: bool,
    },
}

#[derive(Debug, Subcommand)]
enum RuntimeCommand {
    /// Install isolated Python 3.12 and pinned backend dependencies, bootstrapping pinned uv if needed.
    Setup,
    /// Sign into Hugging Face inside MeshHarbor's isolated model cache using an interactive terminal prompt.
    HfLogin,
    /// Download pinned Pixal3D and gated DINOv3 model files (~24 GB) after explicit confirmation.
    InstallModels {
        #[arg(long)]
        confirm_large_download: bool,
        #[arg(long)]
        confirm_gated_access: bool,
    },
    /// Retry the isolated one-time NAF checkpoint conversion after model files are downloaded.
    ConvertNaf,
    /// Install a Linux x86_64 NVIDIA CUDA Python 3.12 runtime and run its CUDA/SDPA preflight.
    CudaSetup,
    /// Sign into Hugging Face inside the isolated Pixal3D CUDA model cache.
    CudaHfLogin,
    /// Download roughly 25–30 GB of pinned CUDA model files after gated DINOv3 access is granted.
    CudaInstallModels {
        #[arg(long)]
        confirm_large_download: bool,
        #[arg(long)]
        confirm_gated_access: bool,
    },
}

#[derive(Clone)]
struct AppState {
    core: OperationCore,
    scheduler: Arc<Semaphore>,
}

#[derive(Clone)]
struct McpServer {
    core: OperationCore,
    scheduler: Arc<Semaphore>,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SimplifyArgs {
    asset_id: String,
    target_triangles: u64,
    #[serde(default = "default_max_error")]
    max_error: f32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MeshProcessArgs {
    asset_id: String,
    /// One of mesh.cleanup, mesh.separate_parts, uv.unwrap, render.optimize, mesh.compress, or mesh.retopology.
    operation: String,
    /// Optional operation parameters, such as resolution/padding for uv.unwrap.
    #[serde(default)]
    parameters: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GenerateArgs {
    asset_id: String,
    #[serde(default = "default_generation_backend")]
    backend_id: String,
    #[serde(default = "default_generation_seed")]
    seed: u64,
    #[serde(default = "default_generation_resolution")]
    resolution: u16,
    #[serde(default = "default_generation_texture_size")]
    texture_size: u16,
    #[serde(default = "default_generation_target_faces")]
    target_faces: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AssetListArgs {
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct JobIdArgs {
    job_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ImportArgs {
    filename: String,
    /// Base64-encoded PNG, JPEG, or GLB bytes.
    bytes_base64: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExportGlbArgs {
    asset_id: String,
    /// Absolute destination path on the computer running the local MCP server.
    /// The file must not already exist; MeshHarbor never overwrites exports.
    output_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExportQuadObjArgs {
    asset_id: String,
    /// Absolute .obj destination path inside the MeshHarbor export directory; must not already exist.
    output_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ModelInstallConfirmation {
    /// Must be true to queue the approximately 25–30 GB model download.
    confirm_large_download: bool,
    /// Must be true only after the owner has obtained/accepted the gated DINOv3 access terms.
    confirm_gated_access: bool,
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    filename: String,
}

#[derive(Debug, Serialize)]
struct ApiErrorBody {
    error: String,
}

#[derive(Debug)]
struct ApiError(CoreError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            CoreError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            CoreError::NotFound(_) => StatusCode::NOT_FOUND,
            CoreError::OperationUnavailable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            CoreError::Conflict(_) => StatusCode::CONFLICT,
            CoreError::Cancelled => StatusCode::CONFLICT,
            CoreError::Io(_) | CoreError::Sql(_) | CoreError::Json(_) | CoreError::Clock => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        (
            status,
            Json(ApiErrorBody {
                error: self.0.to_string(),
            }),
        )
            .into_response()
    }
}

impl From<CoreError> for ApiError {
    fn from(value: CoreError) -> Self {
        Self(value)
    }
}

#[tool_router]
impl McpServer {
    fn new(core: OperationCore, scheduler: Arc<Semaphore>) -> Self {
        Self {
            core,
            scheduler,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List MeshHarbor operations and their implementation status. A listed operation with implemented=false is not available yet."
    )]
    async fn list_operations(&self) -> String {
        serde_json::to_string_pretty(&self.core.list_operations())
            .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
    }

    #[tool(
        description = "Report QuadriFlow sidecar availability and the next step. A managed sidecar is verified using its activation marker and binary digest without being executed."
    )]
    async fn quadriflow_backend_status(&self) -> String {
        serde_json::to_string_pretty(&self.core.quadriflow_backend_status())
            .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
    }

    #[tool(
        description = "Request a guarded installation of the pinned QuadriFlow backend. An in-process verifier checks Sigstore signature/provenance and the executable digest against the trusted manifest before any download or installation; the current manifest is empty, so this request fails closed without network or backend-directory writes. Unsigned temporary Actions artifacts are never installed."
    )]
    async fn install_quadriflow_backend(&self) -> String {
        let core = self.core.clone();
        match task::spawn_blocking(move || core.install_quadriflow_backend()).await {
            Ok(Ok(installed)) => serde_json::to_string_pretty(&installed)
                .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
            Ok(Err(error)) => json!({"error": error.to_string()}).to_string(),
            Err(error) => {
                json!({"error": format!("QuadriFlow installer worker failed: {error}")}).to_string()
            }
        }
    }

    #[tool(description = "List local assets in the MeshHarbor asset store.")]
    async fn list_assets(&self, params: Parameters<AssetListArgs>) -> String {
        let limit = params.0.limit.clamp(1, 200);
        match self.core.store().list_assets(limit) {
            Ok(assets) => serde_json::to_string_pretty(&assets)
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
            Err(error) => format!("{{\"error\":\"{}\"}}", error),
        }
    }

    #[tool(
        description = "Import base64-encoded PNG, JPEG, or GLB bytes into the local MeshHarbor store. This does not download assets or models from the network."
    )]
    async fn import_asset(&self, params: Parameters<ImportArgs>) -> String {
        let decoded = match decode_base64(&params.0.bytes_base64) {
            Ok(bytes) => bytes,
            Err(error) => return json!({"error": error.to_string()}).to_string(),
        };
        let core = self.core.clone();
        let filename = params.0.filename;
        match task::spawn_blocking(move || core.import_asset(&filename, &decoded)).await {
            Ok(Ok(asset)) => serde_json::to_string_pretty(&asset)
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
            Ok(Err(error)) => json!({"error": error.to_string()}).to_string(),
            Err(error) => json!({"error": format!("import worker failed: {error}")}).to_string(),
        }
    }

    #[tool(
        description = "Start a persistent asynchronous mesh.simplify job. Returns the job record immediately; poll get_job for completion."
    )]
    async fn start_simplification(&self, params: Parameters<SimplifyArgs>) -> String {
        let args = params.0;
        match start_simplify_job(
            self.core.clone(),
            self.scheduler.clone(),
            &args.asset_id,
            args.target_triangles,
            args.max_error,
        )
        .await
        {
            Ok(job) => serde_json::to_string_pretty(&job)
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
            Err(error) => json!({"error": error.to_string()}).to_string(),
        }
    }

    #[tool(
        description = "Start a local mesh processing job for mesh.cleanup, mesh.separate_parts, uv.unwrap, render.optimize, mesh.compress, or mesh.retopology. The source asset stays unchanged; poll get_job for the derived asset and report. Part separation emits connected-component primitives in one mesh. uv.unwrap is same-mesh rebaking, not high-to-low projection. Quad retopology requires a compatible managed or adjacent QuadriFlow sidecar and one closed static triangle GLB. Supported embedded base-color/metallic-roughness PNG maps can be projected into the triangulated GLB preview with distance, facing-normal, coverage, and ambiguity gates; the separate quad-topology OBJ remains geometry-only. Unsupported textures/streams fail closed, and no self-intersection or measured geometric-error certification is claimed."
    )]
    async fn start_mesh_processing(&self, params: Parameters<MeshProcessArgs>) -> String {
        let args = params.0;
        match start_mesh_operation_job(
            self.core.clone(),
            self.scheduler.clone(),
            &args.asset_id,
            &args.operation,
            args.parameters.unwrap_or_else(|| json!({})),
        )
        .await
        {
            Ok(job) => serde_json::to_string_pretty(&job)
                .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
            Err(error) => json!({"error": error.to_string()}).to_string(),
        }
    }

    #[tool(
        description = "Start an asynchronous image.generate job using the selected, already prepared local Pixal3D backend (MLX on Apple Silicon or CUDA on Linux/NVIDIA). The `backend_id` can select `auto`, `pixal3d-mlx`, or `pixal3d-cuda`. This tool never downloads models or installs packages; model downloads and gated access are separate user-controlled steps."
    )]
    async fn start_image_generation(&self, params: Parameters<GenerateArgs>) -> String {
        let args = params.0;
        let options = GenerationOptions {
            backend_id: args.backend_id,
            seed: args.seed,
            resolution: args.resolution,
            texture_size: args.texture_size,
            target_faces: args.target_faces,
        };
        match start_generation_job(
            self.core.clone(),
            self.scheduler.clone(),
            &args.asset_id,
            options,
        )
        .await
        {
            Ok(job) => serde_json::to_string_pretty(&job)
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
            Err(error) => json!({"error": error.to_string()}).to_string(),
        }
    }

    #[tool(
        description = "Export a completed mesh asset as a GLB to an absolute path inside MESHHARBOR_EXPORT_DIR (defaults to the MeshHarbor data directory's exports folder) on the local MCP host. The destination must not exist; existing files are never overwritten. Maximum export size is 512 MiB."
    )]
    async fn export_glb(&self, params: Parameters<ExportGlbArgs>) -> String {
        let core = self.core.clone();
        let asset_id = params.0.asset_id;
        let output_path = PathBuf::from(params.0.output_path);
        match task::spawn_blocking(move || export_glb_file(&core, &asset_id, &output_path)).await {
            Ok(Ok(result)) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
            Ok(Err(error)) => json!({"error": error.to_string()}).to_string(),
            Err(error) => {
                json!({"error": format!("GLB export worker failed: {error}")}).to_string()
            }
        }
    }

    #[tool(
        description = "Export a completed QuadriFlow retopology result's editable quad OBJ artifact to an absolute path inside MESHHARBOR_EXPORT_DIR (defaults to the MeshHarbor data directory's exports folder) on the local MCP host. The destination must not exist; existing files are never overwritten. Maximum export size is 64 MiB."
    )]
    async fn export_quad_obj(&self, params: Parameters<ExportQuadObjArgs>) -> String {
        let core = self.core.clone();
        let asset_id = params.0.asset_id;
        let output_path = PathBuf::from(params.0.output_path);
        match task::spawn_blocking(move || export_quad_obj_file(&core, &asset_id, &output_path))
            .await
        {
            Ok(Ok(result)) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
            Ok(Err(error)) => json!({"error": error.to_string()}).to_string(),
            Err(error) => {
                json!({"error": format!("OBJ export worker failed: {error}")}).to_string()
            }
        }
    }

    #[tool(
        description = "Queue the isolated Pixal3D MLX Python 3.12 environment setup. This bootstraps the pinned uv tool when needed, then installs the pinned dependencies. Poll get_job for status."
    )]
    async fn setup_pixal3d_mlx_runtime(&self) -> String {
        match start_runtime_job(
            self.core.clone(),
            self.scheduler.clone(),
            RuntimeJobKind::Setup,
        )
        .await
        {
            Ok(job) => serde_json::to_string_pretty(&job)
                .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
            Err(error) => json!({"error": error.to_string()}).to_string(),
        }
    }

    #[tool(
        description = "Queue the explicit approximately 24 GB Pixal3D/DINOv3 model download. Invoke only when the owner has explicitly confirmed this large download and already obtained the required gated DINOv3 access. No credentials are accepted by this tool; authenticate separately with `meshharbor runtime hf-login`. Poll get_job for status."
    )]
    async fn install_pixal3d_mlx_models(
        &self,
        params: Parameters<ModelInstallConfirmation>,
    ) -> String {
        if !params.0.confirm_large_download || !params.0.confirm_gated_access {
            return json!({
                "error": "Model installation requires explicit confirmation of the approximately 24 GB download and prior owner-approved DINOv3 gated access."
            })
            .to_string();
        }
        match start_runtime_job(
            self.core.clone(),
            self.scheduler.clone(),
            RuntimeJobKind::InstallModels,
        )
        .await
        {
            Ok(job) => serde_json::to_string_pretty(&job)
                .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
            Err(error) => json!({"error": error.to_string()}).to_string(),
        }
    }

    #[tool(
        description = "Queue the isolated one-time NAF checkpoint conversion after the model download succeeds. Poll get_job for status."
    )]
    async fn convert_pixal3d_mlx_naf(&self) -> String {
        match start_runtime_job(
            self.core.clone(),
            self.scheduler.clone(),
            RuntimeJobKind::ConvertNaf,
        )
        .await
        {
            Ok(job) => serde_json::to_string_pretty(&job)
                .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
            Err(error) => json!({"error": error.to_string()}).to_string(),
        }
    }

    #[tool(
        description = "Queue the managed Linux x86_64/NVIDIA CUDA environment setup. It bootstraps pinned uv and managed Python 3.12, resolves upstream dependency ranges into a private per-install lock, installs the PyTorch 2.6.0/CUDA 12.4 profile and runs the CUDA/SDPA preflight. No model weights are downloaded. Poll get_job for status."
    )]
    async fn setup_pixal3d_cuda_runtime(&self) -> String {
        match start_runtime_job(
            self.core.clone(),
            self.scheduler.clone(),
            RuntimeJobKind::CudaSetup,
        )
        .await
        {
            Ok(job) => serde_json::to_string_pretty(&job)
                .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
            Err(error) => json!({"error": error.to_string()}).to_string(),
        }
    }

    #[tool(
        description = "Queue the explicit roughly 25–30 GB Pixal3D/DINOv3/NAF model download for CUDA. Invoke only after the owner explicitly confirms the large download and has obtained gated DINOv3 access. Login is separate via `meshharbor runtime cuda-hf-login`; this tool accepts no credentials. Poll get_job for status."
    )]
    async fn install_pixal3d_cuda_models(
        &self,
        params: Parameters<ModelInstallConfirmation>,
    ) -> String {
        if !params.0.confirm_large_download || !params.0.confirm_gated_access {
            return json!({
                "error": "CUDA model installation requires explicit confirmation of the 25–30 GB download and prior owner-approved DINOv3 access."
            })
            .to_string();
        }
        match start_runtime_job(
            self.core.clone(),
            self.scheduler.clone(),
            RuntimeJobKind::CudaInstallModels,
        )
        .await
        {
            Ok(job) => serde_json::to_string_pretty(&job)
                .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
            Err(error) => json!({"error": error.to_string()}).to_string(),
        }
    }

    #[tool(
        description = "Get the latest state, progress, result, and failure details for a MeshHarbor job."
    )]
    async fn get_job(&self, params: Parameters<JobIdArgs>) -> String {
        match self.core.store().get_job(&params.0.job_id) {
            Ok(Some(job)) => serde_json::to_string_pretty(&job)
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
            Ok(None) => json!({"error":"Job was not found."}).to_string(),
            Err(error) => json!({"error": error.to_string()}).to_string(),
        }
    }

    #[tool(
        description = "Cancel a queued MeshHarbor job. A running native simplification cannot currently be interrupted safely and returns an explicit conflict."
    )]
    async fn cancel_job(&self, params: Parameters<JobIdArgs>) -> String {
        match self.core.store().cancel_job(&params.0.job_id) {
            Ok(job) => serde_json::to_string_pretty(&job)
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
            Err(error) => json!({"error": error.to_string()}).to_string(),
        }
    }

    #[tool(
        description = "Report local runtime capabilities and clearly distinguish ready mesh operations from unavailable model backends."
    )]
    async fn runtime_doctor(&self) -> String {
        serde_json::to_string_pretty(&self.core.runtime_doctor())
            .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
    }
}

#[tool_handler]
impl ServerHandler for McpServer {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("meshharbor=info".parse()?))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let root = cli.data_dir;
    match cli.command {
        Command::Serve { listen } => {
            if !listen.ip().is_loopback() {
                return Err("Refusing non-loopback bind; remote listening is not implemented in this build.".into());
            }
            let token = resolve_api_token(std::env::var("MESHHARBOR_API_TOKEN"))?;
            let _data_dir_lock = data_dir_lock::DataDirLock::try_acquire_for_service(&root)?;
            let core = OperationCore::open(&root)?;
            let app = api_router(
                AppState {
                    core,
                    scheduler: Arc::new(Semaphore::new(1)),
                },
                token.value.clone(),
            );
            let listener = TcpListener::bind(listen).await?;
            eprintln!(
                "MeshHarbor REST API listening at http://{listen}; {}",
                token_startup_log(&token)
            );
            axum::serve(listener, app).await?;
        }
        Command::Mcp => {
            let _data_dir_lock = data_dir_lock::DataDirLock::try_acquire_for_service(&root)?;
            let server = McpServer::new(OperationCore::open(&root)?, Arc::new(Semaphore::new(1)));
            let service = server.serve(rmcp::transport::stdio()).await?;
            service.waiting().await?;
        }
        Command::Import { file } => {
            let bytes = fs::read(&file).await?;
            let name = file
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("asset");
            let asset = OperationCore::open(&root)?.import_asset(name, &bytes)?;
            println!("{}", serde_json::to_string_pretty(&asset)?);
        }
        Command::Inspect { file } => {
            let bytes = fs::read(&file).await?;
            let name = file
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("asset");
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &OperationCore::open(&root)?.inspect_bytes(name, &bytes)?
                )?
            );
        }
        Command::Assets { limit } => {
            let assets = OperationCore::open(&root)?.store().list_assets(limit)?;
            println!("{}", serde_json::to_string_pretty(&assets)?);
        }
        Command::Simplify {
            asset_id,
            target_triangles,
            max_error,
            output,
        } => {
            let core = OperationCore::open(&root)?;
            let result = core.simplify_asset(&asset_id, target_triangles, max_error)?;
            if let Some(path) = output {
                let artifact = result
                    .asset
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.kind == "derived")
                    .ok_or("Simplification output artifact was not found.")?;
                let bytes = core.store().read_artifact(&artifact.id)?.bytes;
                fs::write(path, bytes).await?;
            }
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::ProcessMesh {
            asset_id,
            operation,
            target_quads,
            output,
            quad_output,
        } => {
            let core = OperationCore::open(&root)?;
            if operation != "mesh.retopology" && (target_quads.is_some() || quad_output.is_some()) {
                return Err("--target-quads and --quad-output require mesh.retopology.".into());
            }
            let parameters = if operation == "mesh.retopology" {
                json!({ "target_quads": target_quads.unwrap_or(10_000) })
            } else {
                json!({})
            };
            let result = core.process_mesh_asset(&asset_id, &operation, parameters)?;
            if let Some(path) = output {
                let artifact = result
                    .asset
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.kind == "derived")
                    .ok_or("Mesh processing output artifact was not found.")?;
                let bytes = core.store().read_artifact(&artifact.id)?.bytes;
                fs::write(path, bytes).await?;
            }
            if let Some(path) = quad_output {
                let artifact = result
                    .asset
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.kind == "quad_topology")
                    .ok_or("Quad retopology OBJ artifact was not found.")?;
                let bytes = core.store().read_artifact(&artifact.id)?.bytes;
                fs::write(path, bytes).await?;
            }
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Generate {
            asset_id,
            backend_id,
            seed,
            resolution,
            texture_size,
            target_faces,
            output,
        } => {
            let _runtime_lock = acquire_generation_runtime_lock(&root)?;
            let core = OperationCore::open(&root)?;
            let result = core.generate_pixal3d_image(
                &asset_id,
                GenerationOptions {
                    backend_id,
                    seed,
                    resolution,
                    texture_size,
                    target_faces,
                },
            )?;
            if let Some(path) = output {
                let artifact = result
                    .asset
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.kind == "derived")
                    .ok_or("Generation output artifact was not found.")?;
                let bytes = core.store().read_artifact(&artifact.id)?.bytes;
                fs::write(path, bytes).await?;
            }
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Doctor => println!(
            "{}",
            serde_json::to_string_pretty(&OperationCore::open(&root)?.runtime_doctor())?
        ),
        Command::Backend { command } => {
            let core = OperationCore::open(&root)?;
            match command {
                BackendCommand::Status => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&core.quadriflow_backend_status())?
                    );
                }
                BackendCommand::Install { backend_id: _ } => {
                    let installed = core.install_quadriflow_backend()?;
                    println!("{}", serde_json::to_string_pretty(&installed)?);
                }
            }
        }
        Command::Jobs { command } => match command {
            JobsCommand::RecoverInterrupted { .. } => {
                let _data_dir_lock = data_dir_lock::DataDirLock::try_acquire_for_recovery(&root)?;
                let core = OperationCore::open(&root)?;
                let recovered = core.store().recover_interrupted_jobs()?;
                eprintln!(
                    "Marked {} interrupted job(s) as failed. Existing assets and download caches were left untouched; inspect them before retrying.",
                    recovered.len()
                );
                println!("{}", serde_json::to_string_pretty(&recovered)?);
            }
        },
        Command::Runtime { command } => {
            let _runtime_lock = acquire_runtime_write_lock(&root)?;
            let core = OperationCore::open(&root)?;
            match command {
                RuntimeCommand::Setup => {
                    let report = core.setup_pixal3d_mlx_environment()?;
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
                RuntimeCommand::HfLogin => {
                    core.login_pixal3d_mlx_huggingface()?;
                    println!(
                        "Hugging Face authentication is configured in the isolated MeshHarbor cache."
                    );
                }
                RuntimeCommand::InstallModels {
                    confirm_large_download,
                    confirm_gated_access,
                } => {
                    let report = core
                        .install_pixal3d_mlx_models(confirm_large_download, confirm_gated_access)?;
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
                RuntimeCommand::ConvertNaf => {
                    let report = core.convert_pixal3d_mlx_naf()?;
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
                RuntimeCommand::CudaSetup => {
                    let report = core.setup_pixal3d_cuda_environment()?;
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
                RuntimeCommand::CudaHfLogin => {
                    core.login_pixal3d_cuda_huggingface()?;
                    println!(
                        "Hugging Face authentication is configured in the isolated Pixal3D CUDA cache."
                    );
                }
                RuntimeCommand::CudaInstallModels {
                    confirm_large_download,
                    confirm_gated_access,
                } => {
                    let report = core.install_pixal3d_cuda_models(
                        confirm_large_download,
                        confirm_gated_access,
                    )?;
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
            }
        }
    }
    Ok(())
}

fn api_router(state: AppState, token: String) -> Router {
    Router::new()
        .route("/api/v1/health", get(api_health))
        .route("/api/v1/runtime", get(api_runtime))
        .route(
            "/api/v1/backends/quadriflow",
            get(api_quadriflow_backend_status),
        )
        .route(
            "/api/v1/backends/quadriflow/install",
            post(api_install_quadriflow_backend),
        )
        .route(
            "/api/v1/runtime/pixal3d-mlx/setup",
            post(api_start_runtime_setup),
        )
        .route(
            "/api/v1/runtime/pixal3d-mlx/models",
            post(api_start_runtime_models),
        )
        .route(
            "/api/v1/runtime/pixal3d-mlx/convert-naf",
            post(api_start_runtime_naf_conversion),
        )
        .route(
            "/api/v1/runtime/pixal3d-cuda/setup",
            post(api_start_cuda_runtime_setup),
        )
        .route(
            "/api/v1/runtime/pixal3d-cuda/models",
            post(api_start_cuda_runtime_models),
        )
        .route("/api/v1/operations", get(api_operations))
        .route(
            "/api/v1/assets",
            get(api_list_assets).post(api_import_asset),
        )
        .route("/api/v1/assets/{asset_id}", get(api_get_asset))
        .route("/api/v1/assets/{asset_id}/simplify", post(api_simplify))
        .route("/api/v1/jobs", get(api_list_jobs).post(api_create_job))
        .route("/api/v1/jobs/{job_id}", get(api_get_job))
        .route("/api/v1/jobs/{job_id}/cancel", post(api_cancel_job))
        .route("/api/v1/artifacts/{artifact_id}", get(api_read_artifact))
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .fallback(api_studio_fallback)
        .layer(middleware::from_fn_with_state(token, authenticate))
        .with_state(state)
}

async fn authenticate(State(token): State<String>, request: Request, next: Next) -> Response {
    const DASHBOARD_MARKER: &str = "local-ui-v1";
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    let host_ok = host.is_some_and(is_loopback_authority);
    let origin_ok = request
        .headers()
        .get(header::ORIGIN)
        .map(|value| {
            value
                .to_str()
                .ok()
                .zip(host)
                .is_some_and(|(origin, authority)| is_same_http_origin(origin, authority))
        })
        .unwrap_or(true);
    let bearer_authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|provided| constant_time_equal(provided.as_bytes(), token.as_bytes()));
    let origin_required = is_api_path(request.uri().path())
        && request.method() != Method::GET
        && request.method() != Method::HEAD
        && !bearer_authorized;
    let origin_present = request.headers().contains_key(header::ORIGIN);
    if !host_ok || !origin_ok || (origin_required && !origin_present) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"API requests require a loopback Host; tokenless state-changing requests require an exact same-origin Origin."})),
        )
            .into_response();
    }
    let path = request.uri().path();
    if !is_api_path(path) || path == "/api/v1/health" {
        return next.run(request).await;
    }
    let authorized = bearer_authorized;
    // The built-in Studio is same-origin with this loopback server. Its
    // non-simple marker header prevents HTML forms and cross-origin simple
    // requests from using the tokenless dashboard path. Exact Origin matching
    // above also rejects local services on another port and DNS-rebinding hosts.
    let origin_matches_host = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .zip(host)
        .is_some_and(|(origin, authority)| is_same_http_origin(origin, authority));
    // Browsers omit Origin on some same-origin GETs. Fetch Metadata supplies
    // the browser-enforced provenance in that case; never use this exception
    // for state-changing requests.
    let same_origin_fetch = request
        .headers()
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        == Some("same-origin");
    let safe_read = request.method() == Method::GET || request.method() == Method::HEAD;
    let dashboard_authorized = request
        .headers()
        .get("x-meshharbor-dashboard")
        .and_then(|value| value.to_str().ok())
        == Some(DASHBOARD_MARKER)
        && (origin_matches_host
            || (safe_read && request.headers().get(header::ORIGIN).is_none() && same_origin_fetch));
    if authorized || dashboard_authorized {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(
                json!({"error":"A valid bearer token or same-origin Studio request is required."}),
            ),
        )
            .into_response()
    }
}

async fn api_studio_fallback(request: Request) -> Response {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }
    let request_path = request.uri().path();
    if is_api_path(request_path) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"API route was not found."})),
        )
            .into_response();
    }

    let accepts_html = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html"));
    let asset_path = if request_path == "/" || request_path == "/index.html" {
        "/index.html"
    } else {
        request_path
    };
    let asset = STUDIO_ASSETS
        .iter()
        .find(|asset| asset.path == asset_path)
        .or_else(|| {
            accepts_html
                .then(|| {
                    STUDIO_ASSETS
                        .iter()
                        .find(|asset| asset.path == "/index.html")
                })
                .flatten()
        });

    let Some(asset) = asset else {
        if STUDIO_ASSETS.is_empty() && (request_path == "/" || accepts_html) {
            let response = (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>MeshHarbor Studio</title><body><main><h1>Studio assets are not built</h1><p>Build the client in <code>apps/studio</code>, then rebuild MeshHarbor to embed it.</p></main></body></html>",
            );
            return with_studio_frame_guards(response.into_response());
        }
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        asset
            .content_type
            .parse()
            .unwrap_or(header::HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        asset
            .body
            .len()
            .to_string()
            .parse()
            .unwrap_or(header::HeaderValue::from_static("0")),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    let body = if request.method() == Method::HEAD {
        axum::body::Body::empty()
    } else {
        axum::body::Body::from(asset.body)
    };
    with_studio_frame_guards((headers, body).into_response())
}

fn with_studio_frame_guards(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::HeaderName::from_static("content-security-policy"),
        header::HeaderValue::from_static("frame-ancestors 'none'"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-frame-options"),
        header::HeaderValue::from_static("DENY"),
    );
    response
}

async fn api_health() -> Json<Value> {
    Json(json!({"status":"ok","service":"meshharbor"}))
}
async fn api_runtime(State(state): State<AppState>) -> Json<meshharbor_core::RuntimeDoctor> {
    Json(state.core.runtime_doctor())
}

async fn api_quadriflow_backend_status(
    State(state): State<AppState>,
) -> Json<meshharbor_core::QuadriFlowBackendStatus> {
    Json(state.core.quadriflow_backend_status())
}

async fn api_install_quadriflow_backend(
    State(state): State<AppState>,
) -> Result<Json<meshharbor_core::QuadriFlowInstallResult>, ApiError> {
    let core = state.core;
    let installed = task::spawn_blocking(move || core.install_quadriflow_backend())
        .await
        .map_err(|error| {
            ApiError(CoreError::OperationUnavailable(format!(
                "QuadriFlow installer worker failed: {error}"
            )))
        })??;
    Ok(Json(installed))
}

async fn api_start_runtime_setup(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    let job = start_runtime_job(state.core, state.scheduler, RuntimeJobKind::Setup).await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn api_start_runtime_models(
    State(state): State<AppState>,
    Json(confirmation): Json<ModelInstallConfirmation>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    if !confirmation.confirm_large_download || !confirmation.confirm_gated_access {
        return Err(ApiError(CoreError::InvalidInput(
            "MLX model installation requires explicit confirmation of the large download and prior DINOv3 gated access.".into(),
        )));
    }
    let job = start_runtime_job(state.core, state.scheduler, RuntimeJobKind::InstallModels).await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn api_start_runtime_naf_conversion(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    let job = start_runtime_job(state.core, state.scheduler, RuntimeJobKind::ConvertNaf).await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn api_start_cuda_runtime_setup(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    let job = start_runtime_job(state.core, state.scheduler, RuntimeJobKind::CudaSetup).await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn api_start_cuda_runtime_models(
    State(state): State<AppState>,
    Json(confirmation): Json<ModelInstallConfirmation>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    if !confirmation.confirm_large_download || !confirmation.confirm_gated_access {
        return Err(ApiError(CoreError::InvalidInput(
            "CUDA model installation requires explicit confirmation of the 25–30 GB download and prior DINOv3 gated access.".into(),
        )));
    }
    let job = start_runtime_job(
        state.core,
        state.scheduler,
        RuntimeJobKind::CudaInstallModels,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}
async fn api_operations(
    State(state): State<AppState>,
) -> Json<Vec<meshharbor_core::OperationDescriptor>> {
    Json(state.core.list_operations())
}
async fn api_list_assets(
    State(state): State<AppState>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<Vec<meshharbor_core::Asset>>, ApiError> {
    Ok(Json(
        state.core.store().list_assets(query.limit.unwrap_or(50))?,
    ))
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

async fn api_import_asset(
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    body: Bytes,
) -> Result<Json<meshharbor_core::Asset>, ApiError> {
    if body.len() > 512 * 1024 * 1024 {
        return Err(ApiError(CoreError::InvalidInput(
            "Asset exceeds 512 MiB request limit.".into(),
        )));
    }
    Ok(Json(state.core.import_asset(&query.filename, &body)?))
}

async fn api_get_asset(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
) -> Result<Json<meshharbor_core::Asset>, ApiError> {
    state
        .core
        .store()
        .get_asset(&asset_id)?
        .map(Json)
        .ok_or_else(|| {
            ApiError(CoreError::NotFound(format!(
                "Asset '{asset_id}' was not found."
            )))
        })
}

async fn api_simplify(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
    Json(body): Json<SimplifyBody>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    let job = start_simplify_job(
        state.core,
        state.scheduler,
        &asset_id,
        body.target_triangles,
        body.max_error,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

#[derive(Debug, Deserialize)]
struct SimplifyBody {
    target_triangles: u64,
    #[serde(default = "default_max_error")]
    max_error: f32,
}

#[derive(Debug, Deserialize)]
struct JobStartBody {
    operation: String,
    input: JobStartInput,
    #[serde(default)]
    parameters: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct JobStartInput {
    asset_id: String,
}

async fn api_create_job(
    State(state): State<AppState>,
    Json(body): Json<JobStartBody>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    let parameters = body.parameters.unwrap_or_else(|| json!({}));
    let job = match body.operation.as_str() {
        "mesh.simplify" => {
            let parameters: SimplifyBody = serde_json::from_value(parameters).map_err(|error| {
                ApiError(CoreError::InvalidInput(format!(
                    "Invalid mesh.simplify parameters: {error}"
                )))
            })?;
            start_simplify_job(
                state.core,
                state.scheduler,
                &body.input.asset_id,
                parameters.target_triangles,
                parameters.max_error,
            )
            .await?
        }
        "mesh.cleanup"
        | "mesh.separate_parts"
        | "uv.unwrap"
        | "render.optimize"
        | "mesh.compress"
        | "mesh.retopology" => {
            start_mesh_operation_job(
                state.core,
                state.scheduler,
                &body.input.asset_id,
                &body.operation,
                parameters,
            )
            .await?
        }
        "image.generate" => {
            let parameters: GenerationOptions =
                serde_json::from_value(parameters).map_err(|error| {
                    ApiError(CoreError::InvalidInput(format!(
                        "Invalid image.generate parameters: {error}"
                    )))
                })?;
            start_generation_job(
                state.core,
                state.scheduler,
                &body.input.asset_id,
                parameters,
            )
            .await?
        }
        operation => {
            return Err(ApiError(CoreError::OperationUnavailable(format!(
                "Operation '{operation}' is not implemented by this build."
            ))));
        }
    };
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn api_list_jobs(
    State(state): State<AppState>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<Vec<meshharbor_core::Job>>, ApiError> {
    Ok(Json(
        state.core.store().list_jobs(query.limit.unwrap_or(50))?,
    ))
}

async fn api_get_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<meshharbor_core::Job>, ApiError> {
    state
        .core
        .store()
        .get_job(&job_id)?
        .map(Json)
        .ok_or_else(|| {
            ApiError(CoreError::NotFound(format!(
                "Job '{job_id}' was not found."
            )))
        })
}

async fn api_cancel_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<meshharbor_core::Job>, ApiError> {
    Ok(Json(state.core.store().cancel_job(&job_id)?))
}

async fn start_simplify_job(
    core: OperationCore,
    scheduler: Arc<Semaphore>,
    asset_id: &str,
    target_triangles: u64,
    max_error: f32,
) -> Result<meshharbor_core::Job, CoreError> {
    if core.store().get_asset(asset_id)?.is_none() {
        return Err(CoreError::NotFound(format!(
            "Asset '{asset_id}' was not found."
        )));
    }
    if target_triangles == 0 || !max_error.is_finite() || !(0.0..=1.0).contains(&max_error) {
        return Err(CoreError::InvalidInput(
            "target_triangles must be positive and max_error must be between 0 and 1.".into(),
        ));
    }
    let job = core.store().create_job(
        "mesh.simplify",
        Some(asset_id),
        &json!({"target_triangles":target_triangles,"max_error":max_error}),
    )?;
    let job_id = job.id.clone();
    tokio::spawn(async move {
        let permit = match scheduler.acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return,
        };
        let current = match core.store().get_job(&job_id) {
            Ok(Some(job)) if job.state == meshharbor_core::JobState::Queued => job,
            _ => return,
        };
        if core
            .store()
            .update_job(&job_id, meshharbor_core::JobState::Running, 5, None, None)
            .is_err()
        {
            return;
        }
        let worker_core = core.clone();
        let worker_asset_id = current.input_asset_id.clone().unwrap_or_default();
        let result = task::spawn_blocking(move || {
            worker_core.simplify_asset(&worker_asset_id, target_triangles, max_error)
        })
        .await;
        match result {
            Ok(Ok(result)) => {
                if core
                    .store()
                    .update_job(
                        &job_id,
                        meshharbor_core::JobState::Validating,
                        90,
                        None,
                        None,
                    )
                    .is_ok()
                {
                    let value = json!({ "asset_id": result.asset.id, "report": result.report });
                    let _ = core.store().update_job(
                        &job_id,
                        meshharbor_core::JobState::Succeeded,
                        100,
                        Some(value),
                        None,
                    );
                }
            }
            Ok(Err(error)) => {
                let _ = core.store().update_job(
                    &job_id,
                    meshharbor_core::JobState::Failed,
                    100,
                    None,
                    Some(json!({"message":error.to_string()})),
                );
            }
            Err(error) => {
                let _ = core.store().update_job(
                    &job_id,
                    meshharbor_core::JobState::Failed,
                    100,
                    None,
                    Some(json!({"message":format!("Simplification worker failed: {error}")})),
                );
            }
        }
        drop(permit);
    });
    Ok(job)
}

async fn start_mesh_operation_job(
    core: OperationCore,
    scheduler: Arc<Semaphore>,
    asset_id: &str,
    operation: &str,
    parameters: Value,
) -> Result<meshharbor_core::Job, CoreError> {
    let descriptor = core
        .list_operations()
        .into_iter()
        .find(|item| item.id == operation)
        .ok_or_else(|| {
            CoreError::OperationUnavailable(format!(
                "Mesh operation '{operation}' is not implemented by this build."
            ))
        })?;
    if !descriptor.implemented {
        return Err(CoreError::OperationUnavailable(format!(
            "Mesh operation '{operation}' is not implemented by this build."
        )));
    }
    let source = core
        .store()
        .get_asset(asset_id)?
        .ok_or_else(|| CoreError::NotFound(format!("Asset '{asset_id}' was not found.")))?;
    if source.kind != meshharbor_core::AssetKind::Mesh {
        return Err(CoreError::InvalidInput(
            "Mesh processing requires a previously imported GLB mesh asset.".into(),
        ));
    }
    let job = core
        .store()
        .create_job(operation, Some(asset_id), &parameters)?;
    let job_id = job.id.clone();
    let worker_operation = operation.to_string();
    tokio::spawn(async move {
        let permit = match scheduler.acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return,
        };
        let current = match core.store().get_job(&job_id) {
            Ok(Some(job)) if job.state == meshharbor_core::JobState::Queued => job,
            _ => return,
        };
        if core
            .store()
            .update_job(&job_id, meshharbor_core::JobState::Running, 5, None, None)
            .is_err()
        {
            return;
        }
        let worker_core = core.clone();
        let worker_asset_id = current.input_asset_id.clone().unwrap_or_default();
        let worker_parameters = parameters;
        let operation_for_worker = worker_operation.clone();
        let result = task::spawn_blocking(move || {
            worker_core.process_mesh_asset(
                &worker_asset_id,
                &operation_for_worker,
                worker_parameters,
            )
        })
        .await;
        match result {
            Ok(Ok(result)) => {
                if core
                    .store()
                    .update_job(
                        &job_id,
                        meshharbor_core::JobState::Validating,
                        90,
                        None,
                        None,
                    )
                    .is_ok()
                {
                    let value = json!({
                        "asset_id": result.asset.id,
                        "report": result.report,
                    });
                    let _ = core.store().update_job(
                        &job_id,
                        meshharbor_core::JobState::Succeeded,
                        100,
                        Some(value),
                        None,
                    );
                }
            }
            Ok(Err(error)) => {
                let _ = core.store().update_job(
                    &job_id,
                    meshharbor_core::JobState::Failed,
                    100,
                    None,
                    Some(json!({"message":error.to_string()})),
                );
            }
            Err(error) => {
                let _ = core.store().update_job(
                    &job_id,
                    meshharbor_core::JobState::Failed,
                    100,
                    None,
                    Some(json!({"message":format!("Mesh processing worker failed: {error}")})),
                );
            }
        }
        drop(permit);
    });
    Ok(job)
}

async fn start_generation_job(
    core: OperationCore,
    scheduler: Arc<Semaphore>,
    asset_id: &str,
    options: GenerationOptions,
) -> Result<meshharbor_core::Job, CoreError> {
    let source = core
        .store()
        .get_asset(asset_id)?
        .ok_or_else(|| CoreError::NotFound(format!("Asset '{asset_id}' was not found.")))?;
    if source.kind != meshharbor_core::AssetKind::Image {
        return Err(CoreError::InvalidInput(
            "image.generate requires a previously imported PNG or JPEG image asset.".into(),
        ));
    }
    if !matches!(
        options.backend_id.as_str(),
        "auto" | "pixal3d-mlx" | "pixal3d-cuda"
    ) {
        return Err(CoreError::InvalidInput(
            "backend_id must be auto, pixal3d-mlx, or pixal3d-cuda.".into(),
        ));
    }
    if options.resolution != 512 {
        return Err(CoreError::InvalidInput(
            "Both current Pixal3D backends expose only the reviewed 512 geometry profile.".into(),
        ));
    }
    if !matches!(options.texture_size, 512 | 1024 | 2048) {
        return Err(CoreError::InvalidInput(
            "texture_size must be 512, 1024, or 2048.".into(),
        ));
    }
    if options.target_faces < 4 {
        return Err(CoreError::InvalidInput(
            "target_faces must be at least 4.".into(),
        ));
    }

    let parameters = serde_json::to_value(&options)?;
    let job = core
        .store()
        .create_job("image.generate", Some(asset_id), &parameters)?;
    let job_id = job.id.clone();
    tokio::spawn(async move {
        let permit = match scheduler.acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return,
        };
        let current = match core.store().get_job(&job_id) {
            Ok(Some(job)) if job.state == meshharbor_core::JobState::Queued => job,
            _ => return,
        };
        if core
            .store()
            .update_job(&job_id, meshharbor_core::JobState::Running, 5, None, None)
            .is_err()
        {
            return;
        }

        let worker_core = core.clone();
        let worker_data_dir = core.store().root().to_path_buf();
        let worker_asset_id = current.input_asset_id.clone().unwrap_or_default();
        let result = task::spawn_blocking(move || {
            let _runtime_lock = acquire_generation_runtime_lock(&worker_data_dir)?;
            worker_core.generate_pixal3d_image(&worker_asset_id, options)
        })
        .await;
        match result {
            Ok(Ok(result)) => {
                if core
                    .store()
                    .update_job(
                        &job_id,
                        meshharbor_core::JobState::Validating,
                        90,
                        None,
                        None,
                    )
                    .is_ok()
                {
                    let value = json!({
                        "asset_id": result.asset.id,
                        "backend_id": result.backend_id,
                        "seed": result.seed,
                        "resolution": result.resolution,
                        "texture_size": result.texture_size,
                        "target_faces": result.target_faces,
                        "elapsed_ms": result.elapsed_ms,
                    });
                    let _ = core.store().update_job(
                        &job_id,
                        meshharbor_core::JobState::Succeeded,
                        100,
                        Some(value),
                        None,
                    );
                }
            }
            Ok(Err(error)) => {
                let _ = core.store().update_job(
                    &job_id,
                    meshharbor_core::JobState::Failed,
                    100,
                    None,
                    Some(json!({"message":error.to_string()})),
                );
            }
            Err(error) => {
                let _ = core.store().update_job(
                    &job_id,
                    meshharbor_core::JobState::Failed,
                    100,
                    None,
                    Some(json!({"message":format!("Image generation worker failed: {error}")})),
                );
            }
        }
        drop(permit);
    });
    Ok(job)
}

#[derive(Clone, Copy)]
enum RuntimeJobKind {
    Setup,
    InstallModels,
    ConvertNaf,
    CudaSetup,
    CudaInstallModels,
}

impl RuntimeJobKind {
    fn operation(self) -> &'static str {
        match self {
            Self::Setup => "runtime.pixal3d.setup",
            Self::InstallModels => "runtime.pixal3d.install_models",
            Self::ConvertNaf => "runtime.pixal3d.convert_naf",
            Self::CudaSetup => "runtime.pixal3d.cuda_setup",
            Self::CudaInstallModels => "runtime.pixal3d.cuda_install_models",
        }
    }

    fn backend_id(self) -> &'static str {
        match self {
            Self::Setup | Self::InstallModels | Self::ConvertNaf => "pixal3d-mlx",
            Self::CudaSetup | Self::CudaInstallModels => "pixal3d-cuda",
        }
    }
}

fn map_runtime_lock_error(error: std::io::Error, message: &str) -> CoreError {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        CoreError::Conflict(message.into())
    } else {
        CoreError::Io(error)
    }
}

fn acquire_runtime_write_lock(
    root: &std::path::Path,
) -> Result<data_dir_lock::DataDirLock, CoreError> {
    data_dir_lock::DataDirLock::try_acquire_for_runtime(root).map_err(|error| {
        map_runtime_lock_error(
            error,
            "Another MeshHarbor operation is using managed runtime files or shared caches in this data directory. Wait for it to finish and retry.",
        )
    })
}

fn acquire_generation_runtime_lock(
    root: &std::path::Path,
) -> Result<data_dir_lock::DataDirLock, CoreError> {
    data_dir_lock::DataDirLock::try_acquire_for_generation(root).map_err(|error| {
        map_runtime_lock_error(
            error,
            "A MeshHarbor runtime setup, download, login, or conversion is changing this data directory. Retry generation after it finishes.",
        )
    })
}

async fn start_runtime_job(
    core: OperationCore,
    scheduler: Arc<Semaphore>,
    kind: RuntimeJobKind,
) -> Result<meshharbor_core::Job, CoreError> {
    let job = core.store().create_job(
        kind.operation(),
        None,
        &json!({"backend_id":kind.backend_id()}),
    )?;
    let job_id = job.id.clone();
    tokio::spawn(async move {
        let permit = match scheduler.acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return,
        };
        if !matches!(core.store().get_job(&job_id), Ok(Some(job)) if job.state == meshharbor_core::JobState::Queued)
        {
            return;
        }
        if core
            .store()
            .update_job(&job_id, meshharbor_core::JobState::Running, 5, None, None)
            .is_err()
        {
            return;
        }

        let worker_core = core.clone();
        let worker_data_dir = core.store().root().to_path_buf();
        let result = task::spawn_blocking(move || {
            let _runtime_lock = acquire_runtime_write_lock(&worker_data_dir)?;
            match kind {
                RuntimeJobKind::Setup => worker_core
                    .setup_pixal3d_mlx_environment()
                    .and_then(|report| serde_json::to_value(report).map_err(CoreError::from)),
                RuntimeJobKind::InstallModels => worker_core
                    .install_pixal3d_mlx_models(true, true)
                    .and_then(|report| serde_json::to_value(report).map_err(CoreError::from)),
                RuntimeJobKind::ConvertNaf => worker_core
                    .convert_pixal3d_mlx_naf()
                    .and_then(|report| serde_json::to_value(report).map_err(CoreError::from)),
                RuntimeJobKind::CudaSetup => worker_core
                    .setup_pixal3d_cuda_environment()
                    .and_then(|report| serde_json::to_value(report).map_err(CoreError::from)),
                RuntimeJobKind::CudaInstallModels => worker_core
                    .install_pixal3d_cuda_models(true, true)
                    .and_then(|report| serde_json::to_value(report).map_err(CoreError::from)),
            }
        })
        .await;

        match result {
            Ok(Ok(value)) => {
                if core
                    .store()
                    .update_job(
                        &job_id,
                        meshharbor_core::JobState::Validating,
                        90,
                        None,
                        None,
                    )
                    .is_ok()
                {
                    let _ = core.store().update_job(
                        &job_id,
                        meshharbor_core::JobState::Succeeded,
                        100,
                        Some(value),
                        None,
                    );
                }
            }
            Ok(Err(error)) => {
                let _ = core.store().update_job(
                    &job_id,
                    meshharbor_core::JobState::Failed,
                    100,
                    None,
                    Some(json!({"message":error.to_string()})),
                );
            }
            Err(error) => {
                let _ = core.store().update_job(
                    &job_id,
                    meshharbor_core::JobState::Failed,
                    100,
                    None,
                    Some(json!({"message":format!("Runtime setup worker failed: {error}")})),
                );
            }
        }
        drop(permit);
    });
    Ok(job)
}

async fn api_read_artifact(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
) -> Result<Response, ApiError> {
    let content = state.core.store().read_artifact(&artifact_id)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        content
            .artifact
            .mime_type
            .parse()
            .unwrap_or(header::HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        content
            .bytes
            .len()
            .to_string()
            .parse()
            .unwrap_or(header::HeaderValue::from_static("0")),
    );
    Ok((headers, content.bytes).into_response())
}

const MAX_MCP_EXPORT_BYTES: u64 = 512 * 1024 * 1024;

/// Write a stored mesh GLB to a caller-selected local path without clobbering
/// existing data. This is used only by the explicit MCP export tool.
fn export_glb_file(
    core: &OperationCore,
    asset_id: &str,
    output_path: &std::path::Path,
) -> Result<Value, CoreError> {
    if !output_path.is_absolute() {
        return Err(CoreError::InvalidInput(
            "Export path must be absolute on the local MCP host.".into(),
        ));
    }
    if !output_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("glb"))
    {
        return Err(CoreError::InvalidInput(
            "GLB exports must use a .glb filename.".into(),
        ));
    }
    let asset = core
        .store()
        .get_asset(asset_id)?
        .ok_or_else(|| CoreError::NotFound(format!("Asset '{asset_id}' was not found.")))?;
    if asset.kind != meshharbor_core::AssetKind::Mesh {
        return Err(CoreError::InvalidInput(
            "Only mesh assets can be exported as GLB.".into(),
        ));
    }
    let artifact = asset
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == "source" || artifact.kind == "derived")
        .ok_or_else(|| CoreError::NotFound("Mesh asset has no GLB artifact.".into()))?;
    if artifact.mime_type != "model/gltf-binary" || artifact.byte_size > MAX_MCP_EXPORT_BYTES {
        return Err(CoreError::InvalidInput(
            "Artifact is not a supported GLB or exceeds the 512 MiB MCP export limit.".into(),
        ));
    }

    let file_name = output_path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| CoreError::InvalidInput("Export path must name a file.".into()))?;
    let requested_parent = output_path.parent().ok_or_else(|| {
        CoreError::InvalidInput("Export path must include a parent directory.".into())
    })?;
    let configured_export_root = std::env::var_os("MESHHARBOR_EXPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| core.store().root().join("exports"));
    std::fs::create_dir_all(&configured_export_root)?;
    let canonical_export_root = std::fs::canonicalize(&configured_export_root)?;
    let canonical_parent = std::fs::canonicalize(requested_parent)?;
    if !canonical_parent.is_dir() {
        return Err(CoreError::InvalidInput(
            "Export destination parent must be an existing directory.".into(),
        ));
    }
    if !canonical_parent.starts_with(&canonical_export_root) {
        return Err(CoreError::InvalidInput(format!(
            "MCP exports must stay inside the configured export directory: {}",
            canonical_export_root.display()
        )));
    }
    let destination = canonical_parent.join(file_name);
    let content = core.store().read_artifact(&artifact.id)?;
    if content.bytes.len() as u64 > MAX_MCP_EXPORT_BYTES || !content.bytes.starts_with(b"glTF") {
        return Err(CoreError::InvalidInput(
            "Stored artifact is not a valid-size binary GLB.".into(),
        ));
    }

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                CoreError::Conflict(format!(
                    "Export destination already exists and was not overwritten: {}",
                    destination.display()
                ))
            } else {
                CoreError::Io(error)
            }
        })?;
    let write_result =
        std::io::Write::write_all(&mut file, &content.bytes).and_then(|()| file.sync_all());
    if let Err(error) = write_result {
        drop(file);
        let _ = std::fs::remove_file(&destination);
        return Err(CoreError::Io(error));
    }

    Ok(json!({
        "asset_id": asset.id,
        "artifact_id": artifact.id,
        "output_path": destination,
        "byte_size": content.bytes.len(),
        "sha256": artifact.sha256,
        "overwritten": false
    }))
}

fn export_quad_obj_file(
    core: &OperationCore,
    asset_id: &str,
    output_path: &std::path::Path,
) -> Result<Value, CoreError> {
    const MAX_QUAD_OBJ_EXPORT_BYTES: u64 = 64 * 1024 * 1024;
    if !output_path.is_absolute() {
        return Err(CoreError::InvalidInput(
            "Export path must be absolute on the local MCP host.".into(),
        ));
    }
    if !output_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("obj"))
    {
        return Err(CoreError::InvalidInput(
            "Quad topology exports must use a .obj filename.".into(),
        ));
    }
    let asset = core
        .store()
        .get_asset(asset_id)?
        .ok_or_else(|| CoreError::NotFound(format!("Asset '{asset_id}' was not found.")))?;
    if asset.kind != meshharbor_core::AssetKind::Mesh {
        return Err(CoreError::InvalidInput(
            "Only mesh assets can contain a quad topology artifact.".into(),
        ));
    }
    let artifact = asset
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == "quad_topology")
        .ok_or_else(|| {
            CoreError::NotFound("Mesh asset has no QuadriFlow quad topology OBJ artifact.".into())
        })?;
    if artifact.mime_type != "model/obj" || artifact.byte_size > MAX_QUAD_OBJ_EXPORT_BYTES {
        return Err(CoreError::InvalidInput(
            "Artifact is not a supported OBJ or exceeds the 64 MiB export limit.".into(),
        ));
    }

    let file_name = output_path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| CoreError::InvalidInput("Export path must name a file.".into()))?;
    let requested_parent = output_path.parent().ok_or_else(|| {
        CoreError::InvalidInput("Export path must include a parent directory.".into())
    })?;
    let configured_export_root = std::env::var_os("MESHHARBOR_EXPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| core.store().root().join("exports"));
    std::fs::create_dir_all(&configured_export_root)?;
    let canonical_export_root = std::fs::canonicalize(&configured_export_root)?;
    let canonical_parent = std::fs::canonicalize(requested_parent)?;
    if !canonical_parent.is_dir() {
        return Err(CoreError::InvalidInput(
            "Export destination parent must be an existing directory.".into(),
        ));
    }
    if !canonical_parent.starts_with(&canonical_export_root) {
        return Err(CoreError::InvalidInput(format!(
            "MCP exports must stay inside the configured export directory: {}",
            canonical_export_root.display()
        )));
    }
    let destination = canonical_parent.join(file_name);
    let content = core.store().read_artifact(&artifact.id)?;
    if content.bytes.len() as u64 > MAX_QUAD_OBJ_EXPORT_BYTES
        || std::str::from_utf8(&content.bytes).is_err()
    {
        return Err(CoreError::InvalidInput(
            "Stored quad topology artifact is not a valid-size UTF-8 OBJ.".into(),
        ));
    }

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                CoreError::Conflict(format!(
                    "Export destination already exists and was not overwritten: {}",
                    destination.display()
                ))
            } else {
                CoreError::Io(error)
            }
        })?;
    let write_result =
        std::io::Write::write_all(&mut file, &content.bytes).and_then(|()| file.sync_all());
    if let Err(error) = write_result {
        drop(file);
        let _ = std::fs::remove_file(&destination);
        return Err(CoreError::Io(error));
    }

    Ok(json!({
        "asset_id": asset.id,
        "artifact_id": artifact.id,
        "output_path": destination,
        "byte_size": content.bytes.len(),
        "sha256": artifact.sha256,
        "overwritten": false
    }))
}

fn default_data_dir() -> PathBuf {
    default_data_dir_for(
        std::env::consts::OS,
        |name| std::env::var_os(name),
        std::env::temp_dir(),
    )
}

fn default_data_dir_for(
    os: &str,
    mut read_var: impl FnMut(&str) -> Option<std::ffi::OsString>,
    fallback: PathBuf,
) -> PathBuf {
    fn non_empty_path(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
        value.filter(|path| !path.is_empty()).map(PathBuf::from)
    }

    if let Some(path) = non_empty_path(read_var("MESHHARBOR_HOME")) {
        return path;
    }

    match os {
        "windows" => {
            if let Some(path) = non_empty_path(read_var("LOCALAPPDATA")) {
                return path.join("MeshHarbor");
            }
            if let Some(path) = non_empty_path(read_var("USERPROFILE")) {
                return path.join("AppData").join("Local").join("MeshHarbor");
            }
            if let Some(path) = non_empty_path(read_var("HOME")) {
                return path.join("AppData").join("Local").join("MeshHarbor");
            }
            fallback.join("MeshHarbor")
        }
        "macos" => non_empty_path(read_var("HOME"))
            .map(|path| {
                path.join("Library")
                    .join("Application Support")
                    .join("MeshHarbor")
            })
            .unwrap_or_else(|| {
                fallback
                    .join("Library")
                    .join("Application Support")
                    .join("MeshHarbor")
            }),
        _ => {
            if let Some(path) = non_empty_path(read_var("XDG_DATA_HOME")) {
                return path.join("meshharbor");
            }
            if let Some(path) = non_empty_path(read_var("HOME")) {
                return path.join(".local").join("share").join("meshharbor");
            }
            fallback.join(".local").join("share").join("meshharbor")
        }
    }
}

fn default_max_error() -> f32 {
    0.02
}
fn default_generation_seed() -> u64 {
    GenerationOptions::default().seed
}

fn default_generation_backend() -> String {
    "auto".into()
}
fn default_generation_resolution() -> u16 {
    GenerationOptions::default().resolution
}
fn default_generation_texture_size() -> u16 {
    GenerationOptions::default().texture_size
}
fn default_generation_target_faces() -> u32 {
    GenerationOptions::default().target_faces
}
fn default_limit() -> usize {
    50
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApiTokenSource {
    Generated,
    Configured,
}

struct ApiToken {
    value: String,
    source: ApiTokenSource,
}

fn resolve_api_token(value: Result<String, std::env::VarError>) -> std::io::Result<ApiToken> {
    match value {
        Ok(value) if value.trim().is_empty() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "MESHHARBOR_API_TOKEN must not be empty or whitespace-only; unset it to generate a temporary token.",
        )),
        Ok(value) => Ok(ApiToken {
            value,
            source: ApiTokenSource::Configured,
        }),
        Err(std::env::VarError::NotPresent) => Ok(ApiToken {
            value: uuid::Uuid::new_v4().to_string(),
            source: ApiTokenSource::Generated,
        }),
        Err(std::env::VarError::NotUnicode(_)) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "MESHHARBOR_API_TOKEN must contain valid Unicode text.",
        )),
    }
}

fn token_startup_log(token: &ApiToken) -> String {
    match token.source {
        ApiTokenSource::Generated => format!("temporary bearer token: {}", token.value),
        ApiTokenSource::Configured => {
            "bearer token supplied via MESHHARBOR_API_TOKEN (value not logged)".to_string()
        }
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn is_loopback_authority(authority: &str) -> bool {
    // Do not trim or loosely normalize Host: accepting whitespace or a suffix
    // here makes DNS-rebinding and proxy ambiguity harder to reason about.
    if matches!(
        authority,
        "localhost" | "localhost." | "127.0.0.1" | "[::1]" | "::1"
    ) {
        return true;
    }
    if authority
        .parse::<SocketAddr>()
        .is_ok_and(|address| address.ip().is_loopback())
    {
        return true;
    }
    authority.rsplit_once(':').is_some_and(|(host, port)| {
        port.parse::<u16>().is_ok() && matches!(host, "localhost" | "localhost." | "127.0.0.1")
    })
}

fn is_same_http_origin(origin: &str, authority: &str) -> bool {
    origin.strip_prefix("http://") == Some(authority)
}

fn is_api_path(path: &str) -> bool {
    path == "/api/v1" || path.starts_with("/api/v1/")
}

fn decode_base64(encoded: &str) -> Result<Vec<u8>, String> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let raw = encoded.as_bytes();
    if !raw.len().is_multiple_of(4) {
        return Err("Base64 input length must be divisible by four.".into());
    }
    let mut output = Vec::with_capacity(raw.len() / 4 * 3);
    for (block_index, block) in raw.as_chunks::<4>().0.iter().enumerate() {
        let last = block_index + 1 == raw.len() / 4;
        let mut values = [0u8; 4];
        let mut padding = 0;
        for (index, byte) in block.iter().enumerate() {
            if *byte == b'=' {
                if !last || index < 2 {
                    return Err("Base64 padding is invalid.".into());
                }
                padding += 1;
            } else {
                if padding > 0 {
                    return Err("Base64 data appears after padding.".into());
                }
                values[index] = ALPHABET
                    .iter()
                    .position(|candidate| candidate == byte)
                    .ok_or_else(|| "Base64 contains a non-alphabet character.".to_string())?
                    as u8;
            }
        }
        if padding > 2 {
            return Err("Base64 padding is invalid.".into());
        }
        let n = ((values[0] as u32) << 18)
            | ((values[1] as u32) << 12)
            | ((values[2] as u32) << 6)
            | values[3] as u32;
        output.push((n >> 16) as u8);
        if padding < 2 {
            output.push((n >> 8) as u8);
        }
        if padding == 0 {
            output.push(n as u8);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn mcp_backend_tools_report_unavailable_without_side_effects() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-mcp-backend-{}", uuid::Uuid::new_v4()));
        let server = McpServer::new(
            OperationCore::open(&root).unwrap(),
            Arc::new(Semaphore::new(1)),
        );
        let status: Value =
            serde_json::from_str(&server.quadriflow_backend_status().await).unwrap();
        assert_eq!(status["backend_id"], "quadriflow");
        assert_eq!(status["install_available"], false);
        let install: Value =
            serde_json::from_str(&server.install_quadriflow_backend().await).unwrap();
        assert!(install["error"].as_str().unwrap().contains("QuadriFlow"));
        assert!(!root.join("backends").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backend_cli_exposes_status_and_guarded_install_commands() {
        let status = Cli::try_parse_from(["meshharbor", "backend", "status"]).unwrap();
        assert!(matches!(
            status.command,
            Command::Backend {
                command: BackendCommand::Status
            }
        ));
        let install =
            Cli::try_parse_from(["meshharbor", "backend", "install", "quadriflow"]).unwrap();
        assert!(matches!(
            install.command,
            Command::Backend {
                command: BackendCommand::Install { backend_id }
            } if backend_id == "quadriflow"
        ));
    }

    fn data_dir_for_test(os: &str, vars: &[(&str, &str)]) -> PathBuf {
        use std::{collections::HashMap, ffi::OsString};

        let vars = vars
            .iter()
            .map(|(name, value)| (*name, OsString::from(*value)))
            .collect::<HashMap<_, _>>();
        default_data_dir_for(
            os,
            |name| vars.get(name).cloned(),
            PathBuf::from("/temporary"),
        )
    }

    #[test]
    fn data_directory_override_takes_priority_and_empty_values_are_ignored() {
        assert_eq!(
            data_dir_for_test(
                "windows",
                &[
                    ("MESHHARBOR_HOME", "/explicit"),
                    ("LOCALAPPDATA", "/local-data"),
                ],
            ),
            PathBuf::from("/explicit")
        );
        assert_eq!(
            data_dir_for_test(
                "windows",
                &[("MESHHARBOR_HOME", ""), ("LOCALAPPDATA", "/local-data")],
            ),
            PathBuf::from("/local-data").join("MeshHarbor")
        );
    }

    #[test]
    fn windows_data_directory_falls_back_to_the_user_profile() {
        assert_eq!(
            data_dir_for_test("windows", &[("USERPROFILE", "/profile")]),
            PathBuf::from("/profile")
                .join("AppData")
                .join("Local")
                .join("MeshHarbor")
        );
        assert_eq!(
            data_dir_for_test("windows", &[("LOCALAPPDATA", ""), ("USERPROFILE", "")]),
            PathBuf::from("/temporary").join("MeshHarbor")
        );
    }

    #[test]
    fn macos_and_linux_data_directories_use_their_platform_defaults() {
        assert_eq!(
            data_dir_for_test("macos", &[("HOME", "/profile")]),
            PathBuf::from("/profile")
                .join("Library")
                .join("Application Support")
                .join("MeshHarbor")
        );
        assert_eq!(
            data_dir_for_test("linux", &[("XDG_DATA_HOME", "/xdg")]),
            PathBuf::from("/xdg").join("meshharbor")
        );
        assert_eq!(
            data_dir_for_test("linux", &[("XDG_DATA_HOME", ""), ("HOME", "/profile")]),
            PathBuf::from("/profile")
                .join(".local")
                .join("share")
                .join("meshharbor")
        );
    }

    #[test]
    fn runtime_lock_conflicts_are_retryable_and_services_can_coexist() {
        let _lock_test_guard = DATA_DIR_LOCK_TEST_MUTEX
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root =
            std::env::temp_dir().join(format!("meshharbor-runtime-lock-{}", uuid::Uuid::new_v4()));
        let service = data_dir_lock::DataDirLock::try_acquire_for_service(&root).unwrap();
        let installer = acquire_runtime_write_lock(&root).unwrap();

        let generation_error = acquire_generation_runtime_lock(&root).unwrap_err();
        assert!(matches!(
            generation_error,
            CoreError::Conflict(message) if message.contains("Retry generation after it finishes")
        ));
        drop(installer);

        let first_reader = acquire_generation_runtime_lock(&root).unwrap();
        let second_reader = acquire_generation_runtime_lock(&root).unwrap();
        let install_error = acquire_runtime_write_lock(&root).unwrap_err();
        assert!(matches!(
            install_error,
            CoreError::Conflict(message) if message.contains("Wait for it to finish and retry")
        ));
        drop(first_reader);
        drop(second_reader);

        let installer_after_readers = acquire_runtime_write_lock(&root).unwrap();
        drop(installer_after_readers);
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_job_recovery_requires_an_explicit_stopped_process_acknowledgement() {
        assert!(Cli::try_parse_from(["meshharbor", "jobs", "recover-interrupted"]).is_err());
        assert!(
            Cli::try_parse_from([
                "meshharbor",
                "jobs",
                "recover-interrupted",
                "--confirm-no-running-meshharbor",
            ])
            .is_ok()
        );
    }

    #[test]
    fn validates_base64_padding() {
        assert_eq!(decode_base64("dGVzdA==").unwrap(), b"test");
        assert!(decode_base64("test===").is_err());
        assert!(decode_base64("dGV$").is_err());
    }

    #[test]
    fn api_token_environment_override_rejects_empty_or_whitespace_values() {
        for value in ["", " ", "\t\n"] {
            let error = match resolve_api_token(Ok(value.to_string())) {
                Ok(_) => panic!("empty token override was accepted"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("must not be empty"));
        }
    }

    #[test]
    fn api_token_fallback_and_configured_value_have_distinct_log_behavior() {
        let generated = resolve_api_token(Err(std::env::VarError::NotPresent))
            .unwrap_or_else(|_| panic!("missing override should generate a temporary token"));
        assert_eq!(generated.source, ApiTokenSource::Generated);
        assert!(!generated.value.is_empty());
        assert!(
            token_startup_log(&generated).contains(&generated.value),
            "the generated temporary token must be available to a local API client"
        );

        let configured = resolve_api_token(Ok("custom-secret-token".to_string()))
            .unwrap_or_else(|_| panic!("non-empty override should be accepted"));
        assert_eq!(configured.value, "custom-secret-token");
        assert_eq!(configured.source, ApiTokenSource::Configured);
        assert_eq!(
            token_startup_log(&configured),
            "bearer token supplied via MESHHARBOR_API_TOKEN (value not logged)"
        );
        assert!(!token_startup_log(&configured).contains(&configured.value));
    }

    #[test]
    fn non_unicode_api_token_override_fails_without_echoing_the_value() {
        let error = resolve_api_token(Err(std::env::VarError::NotUnicode(
            std::ffi::OsString::from("secret"),
        )))
        .err()
        .expect("non-Unicode overrides must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn api_token_comparison_checks_full_value() {
        assert!(constant_time_equal(b"a token", b"a token"));
        assert!(!constant_time_equal(b"a token", b"a tokeN"));
        assert!(!constant_time_equal(b"a", b"aa"));
    }

    #[test]
    fn studio_origin_must_match_the_exact_loopback_authority() {
        assert!(is_same_http_origin(
            "http://127.0.0.1:7432",
            "127.0.0.1:7432"
        ));
        assert!(is_same_http_origin(
            "http://localhost:7432",
            "localhost:7432"
        ));
        assert!(!is_same_http_origin(
            "http://localhost:5173",
            "localhost:7432"
        ));
        assert!(!is_same_http_origin(
            "https://localhost:7432",
            "localhost:7432"
        ));
        assert!(!is_same_http_origin(
            "http://localhost:7432.evil.test",
            "localhost:7432"
        ));
    }

    #[test]
    fn host_authority_accepts_only_loopback_names_and_numeric_ports() {
        for authority in [
            "localhost",
            "localhost:7432",
            "localhost.:7432",
            "127.0.0.1",
            "127.0.0.1:7432",
            "[::1]:7432",
        ] {
            assert!(is_loopback_authority(authority), "expected {authority:?}");
        }
        for authority in [
            "localhost.evil.test:7432",
            "notlocalhost:7432",
            "127.0.0.1.evil.test:7432",
            "localhost:7432.evil.test",
            "localhost:7432@evil.test",
            " localhost:7432",
            "localhost:7432 ",
            "localhost:bad",
        ] {
            assert!(
                !is_loopback_authority(authority),
                "unexpectedly accepted {authority:?}"
            );
        }
    }

    #[tokio::test]
    async fn rest_api_allows_health_and_same_origin_dashboard_but_rejects_unproven_requests() {
        let root = std::env::temp_dir().join(format!("meshharbor-api-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let app = api_router(
            AppState {
                core,
                scheduler: Arc::new(Semaphore::new(1)),
            },
            "correct horse".into(),
        );

        let health = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/health")
                    .header(header::HOST, "localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        let denied = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/assets")
                    .header(header::HOST, "localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let dashboard = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/runtime")
                    .header(header::HOST, "localhost:7432")
                    .header(header::ORIGIN, "http://localhost:7432")
                    .header("x-meshharbor-dashboard", "local-ui-v1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(dashboard.status(), StatusCode::OK);

        let backend_status = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/backends/quadriflow")
                    .header(header::HOST, "localhost:7432")
                    .header(header::ORIGIN, "http://localhost:7432")
                    .header("x-meshharbor-dashboard", "local-ui-v1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(backend_status.status(), StatusCode::OK);
        let backend_body = backend_status
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let backend_json: Value = serde_json::from_slice(&backend_body).unwrap();
        assert_eq!(backend_json["backend_id"], "quadriflow");
        assert_eq!(backend_json["install_available"], false);

        let backend_install = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/backends/quadriflow/install")
                    .header(header::HOST, "localhost:7432")
                    .header(header::ORIGIN, "http://localhost:7432")
                    .header("x-meshharbor-dashboard", "local-ui-v1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(backend_install.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let error_body = backend_install
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let error_json: Value = serde_json::from_slice(&error_body).unwrap();
        assert!(error_json["error"].as_str().unwrap().contains("QuadriFlow"));
        assert!(!root.join("backends").exists());

        let missing_marker = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/runtime")
                    .header(header::HOST, "localhost:7432")
                    .header(header::ORIGIN, "http://localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_marker.status(), StatusCode::UNAUTHORIZED);

        let missing_origin = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/runtime")
                    .header(header::HOST, "localhost:7432")
                    .header("x-meshharbor-dashboard", "local-ui-v1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_origin.status(), StatusCode::UNAUTHORIZED);

        let originless_dashboard_post = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/assets?filename=originless.png")
                    .header(header::HOST, "localhost:7432")
                    .header("x-meshharbor-dashboard", "local-ui-v1")
                    .body(Body::from(b"not an image".to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(originless_dashboard_post.status(), StatusCode::FORBIDDEN);

        let originless_bearer_post = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/assets?filename=originless.png")
                    .header(header::HOST, "localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer correct horse")
                    .body(Body::from(b"\x89PNG\r\n\x1a\ndashboard".to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(originless_bearer_post.status(), StatusCode::OK);

        let cross_origin_options = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("OPTIONS")
                    .uri("/api/v1/jobs")
                    .header(header::HOST, "localhost:7432")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "x-meshharbor-dashboard",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cross_origin_options.status(), StatusCode::FORBIDDEN);

        let cross_origin_post = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/jobs")
                    .header(header::HOST, "localhost:7432")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .header(header::AUTHORIZATION, "Bearer correct horse")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cross_origin_post.status(), StatusCode::FORBIDDEN);

        let untrusted_host = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/runtime")
                    .header(header::HOST, "localhost.evil.test:7432")
                    .header(header::ORIGIN, "http://localhost.evil.test:7432")
                    .header(header::AUTHORIZATION, "Bearer correct horse")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(untrusted_host.status(), StatusCode::FORBIDDEN);

        let same_origin_fetch = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/runtime")
                    .header(header::HOST, "localhost:7432")
                    .header("sec-fetch-site", "same-origin")
                    .header("x-meshharbor-dashboard", "local-ui-v1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(same_origin_fetch.status(), StatusCode::OK);

        let dashboard_upload = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/assets?filename=dashboard.png")
                    .header(header::HOST, "localhost:7432")
                    .header(header::ORIGIN, "http://localhost:7432")
                    .header("x-meshharbor-dashboard", "local-ui-v1")
                    .body(Body::from(b"\x89PNG\r\n\x1a\ndashboard".to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(dashboard_upload.status(), StatusCode::OK);

        let different_loopback_origin = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/runtime")
                    .header(header::HOST, "localhost:7432")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .header("x-meshharbor-dashboard", "local-ui-v1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(different_loopback_origin.status(), StatusCode::FORBIDDEN);

        let upload = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/assets?filename=sample.png")
                    .header(header::HOST, "localhost:7432")
                    .header(header::ORIGIN, "http://localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer correct horse")
                    .body(Body::from(b"\x89PNG\r\n\x1a\nexample".to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(upload.status(), StatusCode::OK);
        let body = upload.into_body().collect().await.unwrap().to_bytes();
        let asset: Value = serde_json::from_slice(&body).unwrap();
        let asset_id = asset["id"].as_str().unwrap();

        let fetched = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/v1/assets/{asset_id}"))
                    .header(header::HOST, "localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer correct horse")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(fetched.status(), StatusCode::OK);
        drop(fetched);

        let bad_origin = api_router(
            AppState {
                core: OperationCore::open(&root).unwrap(),
                scheduler: Arc::new(Semaphore::new(1)),
            },
            "correct horse".into(),
        )
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/runtime")
                .header(header::HOST, "localhost:7432")
                .header(header::ORIGIN, "https://attacker.example")
                .header(header::AUTHORIZATION, "Bearer correct horse")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(bad_origin.status(), StatusCode::FORBIDDEN);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn studio_route_is_loopback_only_and_does_not_expose_api_without_a_token() {
        let root = std::env::temp_dir().join(format!("meshharbor-studio-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let app = api_router(
            AppState {
                core,
                scheduler: Arc::new(Semaphore::new(1)),
            },
            "secret".into(),
        );
        let studio = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .header(header::HOST, "127.0.0.1:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        if STUDIO_ASSETS.is_empty() {
            assert_eq!(studio.status(), StatusCode::SERVICE_UNAVAILABLE);
        } else {
            assert_eq!(studio.status(), StatusCode::OK);
            assert_eq!(
                studio.headers().get(header::CONTENT_TYPE).unwrap(),
                "text/html; charset=utf-8"
            );
        }
        assert_eq!(
            studio.headers().get("content-security-policy").unwrap(),
            "frame-ancestors 'none'"
        );
        assert_eq!(studio.headers().get("x-frame-options").unwrap(), "DENY");

        let denied_api = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/not-a-route")
                    .header(header::HOST, "127.0.0.1:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied_api.status(), StatusCode::UNAUTHORIZED);

        let denied_api_prefix = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1")
                    .header(header::HOST, "127.0.0.1:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied_api_prefix.status(), StatusCode::UNAUTHORIZED);

        let forbidden_origin = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .header(header::HOST, "127.0.0.1:7432")
                    .header(header::ORIGIN, "https://attacker.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden_origin.status(), StatusCode::FORBIDDEN);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn minimal_triangle_glb() -> Vec<u8> {
        let mut binary = Vec::new();
        for position in [
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
        ] {
            for component in position {
                binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        let index_offset = binary.len();
        for index in [0u16, 2, 1, 1, 2, 3] {
            binary.extend_from_slice(&index.to_le_bytes());
        }
        let document = json!({
            "asset":{"version":"2.0"},
            "buffers":[{"byteLength":binary.len()}],
            "bufferViews":[
                {"buffer":0,"byteOffset":0,"byteLength":48,"byteStride":12,"target":34962},
                {"buffer":0,"byteOffset":index_offset,"byteLength":12,"target":34963}
            ],
            "accessors":[
                {"bufferView":0,"componentType":5126,"count":4,"type":"VEC3"},
                {"bufferView":1,"componentType":5123,"count":6,"type":"SCALAR"}
            ],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}]
        });
        let mut json_bytes = serde_json::to_vec(&document).unwrap();
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        while !binary.len().is_multiple_of(4) {
            binary.push(0);
        }
        let total_len = 12 + 8 + json_bytes.len() + 8 + binary.len();
        let mut bytes = Vec::with_capacity(total_len);
        bytes.extend_from_slice(b"glTF");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&(total_len as u32).to_le_bytes());
        bytes.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0x4e4f_534au32.to_le_bytes());
        bytes.extend_from_slice(&json_bytes);
        bytes.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0x004e_4942u32.to_le_bytes());
        bytes.extend_from_slice(&binary);
        bytes
    }

    #[tokio::test]
    async fn api_job_returns_accepted_and_completes_against_the_shared_core() {
        let root = std::env::temp_dir().join(format!("meshharbor-job-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let source = core
            .import_asset("square.glb", &minimal_triangle_glb())
            .unwrap();
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: Arc::new(Semaphore::new(1)),
            },
            "token".into(),
        );
        let body = json!({
            "operation":"mesh.simplify",
            "input":{"asset_id":source.id},
            "parameters":{"target_triangles":1,"max_error":1.0}
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/jobs")
                    .header(header::HOST, "localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let submitted: meshharbor_core::Job = serde_json::from_slice(&body).unwrap();
        assert_eq!(submitted.state, meshharbor_core::JobState::Queued);

        let mut final_job = submitted;
        for _ in 0..100 {
            final_job = core.store().get_job(&final_job.id).unwrap().unwrap();
            if final_job.state.is_terminal() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            final_job.state,
            meshharbor_core::JobState::Succeeded,
            "job details: {final_job:?}"
        );
        assert!(
            final_job
                .result
                .as_ref()
                .and_then(|value| value.get("asset_id"))
                .is_some()
        );
        assert_eq!(core.store().list_assets(10).unwrap().len(), 2);

        let listed_response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/jobs?limit=20")
                    .header(header::HOST, "localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed_response.status(), StatusCode::OK);
        let listed: Vec<meshharbor_core::Job> = serde_json::from_slice(
            &listed_response.into_body().collect().await.unwrap().to_bytes(),
        )
        .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, final_job.id);
        assert_eq!(listed[0].state, meshharbor_core::JobState::Succeeded);
        assert_eq!(
            core.store().get_job(&final_job.id).unwrap().unwrap().state,
            meshharbor_core::JobState::Succeeded,
            "listing jobs must not mutate their persisted state"
        );
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn image_generation_job_route_validates_the_reviewed_profile_without_starting_inference()
    {
        let root =
            std::env::temp_dir().join(format!("meshharbor-generate-api-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let source = core
            .import_asset("source.png", b"\x89PNG\r\n\x1a\nlocal-test")
            .unwrap();
        let app = api_router(
            AppState {
                core,
                scheduler: Arc::new(Semaphore::new(1)),
            },
            "token".into(),
        );
        let body = json!({
            "operation":"image.generate",
            "input":{"asset_id":source.id},
            "parameters":{"resolution":1024}
        })
        .to_string();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/jobs")
                    .header(header::HOST, "localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            serde_json::from_slice::<Value>(
                &response.into_body().collect().await.unwrap().to_bytes()
            )
            .unwrap()["error"],
            "invalid request: Both current Pixal3D backends expose only the reviewed 512 geometry profile."
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mcp_glb_export_writes_only_to_new_absolute_glb_paths() {
        let root = std::env::temp_dir().join(format!("meshharbor-export-{}", uuid::Uuid::new_v4()));
        let output_dir = root.join("store/exports");
        std::fs::create_dir_all(&output_dir).unwrap();
        let core = OperationCore::open(root.join("store")).unwrap();
        let source_bytes = minimal_triangle_glb();
        let asset = core.import_asset("result.glb", &source_bytes).unwrap();

        let destination = output_dir.join("result.glb");
        let exported = export_glb_file(&core, &asset.id, &destination).unwrap();
        assert_eq!(exported["overwritten"], false);
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            source_bytes,
            "export must be byte-identical to the stored GLB"
        );

        let conflict = export_glb_file(&core, &asset.id, &destination).unwrap_err();
        assert!(matches!(conflict, CoreError::Conflict(_)));
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            source_bytes,
            "a repeated export must leave the existing file untouched"
        );

        let relative =
            export_glb_file(&core, &asset.id, std::path::Path::new("result.glb")).unwrap_err();
        assert!(matches!(relative, CoreError::InvalidInput(_)));

        let wrong_extension =
            export_glb_file(&core, &asset.id, &output_dir.join("result.obj")).unwrap_err();
        assert!(matches!(wrong_extension, CoreError::InvalidInput(_)));

        let outside_dir = root.join("outside");
        std::fs::create_dir_all(&outside_dir).unwrap();
        let escaped =
            export_glb_file(&core, &asset.id, &outside_dir.join("escape.glb")).unwrap_err();
        assert!(matches!(escaped, CoreError::InvalidInput(_)));
        assert!(!outside_dir.join("escape.glb").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let symlinked_dir = output_dir.join("outside-link");
            symlink(&outside_dir, &symlinked_dir).unwrap();
            let through_symlink =
                export_glb_file(&core, &asset.id, &symlinked_dir.join("symlink-escape.glb"))
                    .unwrap_err();
            assert!(matches!(through_symlink, CoreError::InvalidInput(_)));
            assert!(!outside_dir.join("symlink-escape.glb").exists());
        }
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mcp_quad_obj_export_preserves_topology_bytes_and_never_overwrites() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-quad-export-{}", uuid::Uuid::new_v4()));
        let output_dir = root.join("store/exports");
        std::fs::create_dir_all(&output_dir).unwrap();
        let core = OperationCore::open(root.join("store")).unwrap();
        let asset = core
            .import_asset("result.glb", &minimal_triangle_glb())
            .unwrap();
        let obj = b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n";
        core.store()
            .add_artifact(&asset.id, "quad_topology", "model/obj", obj)
            .unwrap();

        let destination = output_dir.join("result.obj");
        let exported = export_quad_obj_file(&core, &asset.id, &destination).unwrap();
        assert_eq!(exported["overwritten"], false);
        assert_eq!(std::fs::read(&destination).unwrap(), obj);

        let conflict = export_quad_obj_file(&core, &asset.id, &destination).unwrap_err();
        assert!(matches!(conflict, CoreError::Conflict(_)));
        assert_eq!(std::fs::read(&destination).unwrap(), obj);

        let wrong_extension =
            export_quad_obj_file(&core, &asset.id, &output_dir.join("result.glb")).unwrap_err();
        assert!(matches!(wrong_extension, CoreError::InvalidInput(_)));

        let outside_dir = root.join("outside");
        std::fs::create_dir_all(&outside_dir).unwrap();
        let escaped =
            export_quad_obj_file(&core, &asset.id, &outside_dir.join("escape.obj")).unwrap_err();
        assert!(matches!(escaped, CoreError::InvalidInput(_)));
        assert!(!outside_dir.join("escape.obj").exists());

        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mcp_glb_export_rejects_images_without_creating_files() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-export-image-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let core = OperationCore::open(root.join("store")).unwrap();
        let image = core
            .import_asset("source.png", b"\x89PNG\r\n\x1a\nexport-test")
            .unwrap();
        let destination = root.join("store/exports/not-a-mesh.glb");
        let error = export_glb_file(&core, &image.id, &destination).unwrap_err();
        assert!(matches!(error, CoreError::InvalidInput(_)));
        assert!(!destination.exists());
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }
}
