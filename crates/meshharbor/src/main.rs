use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::http::Method;
use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::DefaultBodyLimit,
    extract::rejection::JsonRejection,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use clap::{Parser, Subcommand};
use huggingface_status::HuggingFaceStatuses;
use job_scheduler::JobScheduler;
use meshharbor_core::{
    CoreError, GenerationInput, GenerationOptions, MAX_ASSET_BYTES, MODEL_GENERATION_OPERATION,
    OperationCore, Pixal3dProfile, RuntimeDoctor, RuntimeJobKind,
};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{fs, io::AsyncReadExt, net::TcpListener, task};
use tracing_subscriber::EnvFilter;

include!(concat!(env!("OUT_DIR"), "/studio_assets.rs"));

mod actions;
mod blender_bridge;
mod browser_login;
mod data_dir_lock;
mod engine_client;
mod huggingface_status;
mod job_scheduler;
mod openapi;
#[cfg(feature = "bundled-quadriflow")]
mod quadriflow_worker;
mod service;
mod uninstall;

const DEFAULT_LISTEN: &str = "127.0.0.1:7432";
type BoxError = Box<dyn std::error::Error + Send + Sync>;
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
        #[arg(long, default_value = DEFAULT_LISTEN)]
        listen: SocketAddr,
        #[cfg(windows)]
        #[arg(long, hide = true)]
        background_id: Option<uuid::Uuid>,
    },
    /// Manage the background REST API service for the current user.
    Service {
        #[command(subcommand)]
        command: service::ServiceCommand,
    },
    /// Preview or permanently remove this MeshHarbor installation and its local data.
    Uninstall {
        /// Permanently delete the listed files and remove the matching background service.
        #[arg(long)]
        confirm: bool,
        /// Leave the running executable in place (for example, for Cargo to uninstall).
        #[arg(long)]
        keep_binary: bool,
    },
    /// Start the stdio MCP server.
    Mcp,
    /// Control the same running engine, job queue, and models as Studio.
    Engine {
        /// Loopback address of the running MeshHarbor server.
        #[arg(long, global = true, env = engine_client::SERVER_ENV, default_value = DEFAULT_LISTEN, value_parser = engine_client::parse_server)]
        server: SocketAddr,
        #[command(subcommand)]
        command: engine_client::Command,
    },
    /// Install and enable the add-on on this machine with Blender closed.
    InstallBlenderAddon {
        #[arg(long, env = engine_client::SERVER_ENV, default_value = DEFAULT_LISTEN, value_parser = engine_client::parse_server)]
        server: SocketAddr,
    },
    /// Run offline against the explicitly selected local data directory.
    Local {
        #[command(subcommand)]
        command: LocalCommand,
    },
    /// Mark unfinished jobs as failed after stopping all engine processes for this data directory.
    RecoverJobs {
        #[arg(long, required = true, action = clap::ArgAction::SetTrue)]
        confirm_no_running_meshharbor: bool,
    },
}

#[derive(Debug, Subcommand)]
enum LocalCommand {
    /// Import a PNG, JPEG, or GLB asset (up to 512 MiB; images up to 16 MiB/16 MP).
    Import { file: PathBuf },
    /// Inspect a supported asset file (up to 512 MiB; images up to 16 MiB/16 MP).
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
        /// Write a new output file; existing files are never overwritten.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Check an imported GLB for retopology without installing or running a backend.
    RetopologyPreflight {
        asset_id: String,
        /// Approximate requested quad count (4 to 50,000); omit for automatic density.
        #[arg(long)]
        target_quads: Option<u32>,
    },
    /// Run a supported local mesh operation. For retopology, --output writes the triangle GLB preview and --quad-output preserves the editable OBJ quads.
    ProcessMesh {
        asset_id: String,
        #[arg(long, value_parser = ["mesh.cleanup", "mesh.voxel_remesh", "uv.unwrap", "render.optimize", "mesh.compress", "mesh.retopology"])]
        operation: String,
        /// Approximate quad count for mesh.retopology (4 to 50,000); omit for automatic density.
        #[arg(long)]
        target_quads: Option<u32>,
        /// xatlas packing resolution hint for uv.unwrap (64 to 4,096).
        #[arg(long, value_parser = clap::value_parser!(u32).range(64..=4096))]
        resolution: Option<u32>,
        /// Grid resolution for mesh.voxel_remesh (16 to 160, default 96).
        #[arg(long, value_parser = clap::value_parser!(u32).range(
            i64::from(meshharbor_core::MIN_VOXEL_RESOLUTION)..=i64::from(meshharbor_core::MAX_VOXEL_RESOLUTION)
        ))]
        voxel_resolution: Option<u32>,
        /// Chart gutter in atlas texels for uv.unwrap (0 to 64).
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=64))]
        padding: Option<u32>,
        /// Overdraw optimization threshold for render.optimize (1.0 to 1.5).
        #[arg(long, value_parser = parse_overdraw_threshold)]
        overdraw_threshold: Option<f32>,
        /// Remove duplicate faces for retopology without simplifying the mesh. Requires mesh.cleanup.
        #[arg(long)]
        for_retopology: bool,
        /// Optional new path for a derived GLB or the triangulated retopology preview; existing files are never overwritten.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Optional new path for the original editable quad OBJ from mesh.retopology; existing files are never overwritten.
        #[arg(long)]
        quad_output: Option<PathBuf>,
    },
    /// Generate a GLB using a ready local backend that supports the input.
    Generate {
        #[arg(required_unless_present = "prompt")]
        asset_id: Option<String>,
        #[arg(
            long,
            conflicts_with = "asset_id",
            help = "Text prompt for a backend that supports text-to-3D"
        )]
        prompt: Option<String>,
        #[arg(long, default_value = "auto", value_parser = ["auto", "pixal3d-mlx", "pixal3d-cuda", "trellis2-mlx", "trellis2-cuda", "triposg-mlx", "triposg-cuda", "trellis-text-cuda", "trellis-text-mlx", "lato2-cuda", "lato2-mlx"])]
        backend_id: String,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long, default_value_t = 512)]
        resolution: u16,
        #[arg(long, default_value_t = 1024)]
        texture_size: u16,
        #[arg(long, default_value_t = 100_000)]
        target_faces: u32,
        /// Approximate vertex target for LATO.2.
        #[arg(long, value_parser = clap::value_parser!(u32).range(200..=5000))]
        target_vertices: Option<u32>,
        /// Additional backend catalog settings as a JSON object.
        #[arg(long, value_parser = parse_cli_json::<std::collections::BTreeMap<String, Value>>)]
        tuning: Option<std::collections::BTreeMap<String, Value>>,
        /// Posed multiview input as a JSON object; positional asset ID must match the first view.
        #[arg(long, conflicts_with = "prompt", value_parser = parse_cli_json::<meshharbor_core::PosedGenerationInput>)]
        view_input: Option<meshharbor_core::PosedGenerationInput>,
        /// Write a new output file; existing files are never overwritten.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Show implemented operations and detected runtime capabilities.
    Doctor,
}

#[derive(Clone)]
struct AppState {
    core: OperationCore,
    scheduler: Arc<JobScheduler>,
}

#[derive(Clone)]
struct McpServer {
    data_dir: PathBuf,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImportArgs {
    filename: String,
    /// Base64-encoded PNG, JPEG, or GLB bytes.
    bytes_base64: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BlenderInstallArgs {
    /// Set true only after the owner approves updating the local Blender preferences.
    confirmed: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UploadQuery {
    filename: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
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
    fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Upload base64-encoded PNG, JPEG, or GLB bytes into the running Studio engine. Large local files can be uploaded with the CLI without base64."
    )]
    async fn import_engine_asset(&self, params: Parameters<ImportArgs>) -> CallToolResult {
        engine_tool_result(
            async {
                let bytes = decode_base64(&params.0.bytes_base64)?;
                engine_client::Client::configured()?
                    .import_asset(&params.0.filename, bytes)
                    .await
            }
            .await,
        )
    }

    #[tool(
        description = "Generate a new 3D model from a stored image or text in the running Studio engine. Returns its job ID."
    )]
    async fn generate_model(&self, params: Parameters<actions::GenerateModel>) -> CallToolResult {
        engine_tool_result(
            async {
                engine_client::Client::configured()?
                    .generate(params.0)
                    .await
            }
            .await,
        )
    }

    #[tool(
        description = "Install and enable the Blender add-on on this machine after explicit owner approval. Blender must be closed."
    )]
    async fn install_blender_addon(
        &self,
        params: Parameters<BlenderInstallArgs>,
    ) -> CallToolResult {
        engine_tool_result(
            async {
                if !params.0.confirmed {
                    return Err(
                        "Installing the Blender add-on requires explicit owner approval.".into(),
                    );
                }
                let server = engine_client::configured_address()?;
                Ok(serde_json::to_value(
                    blender_bridge::install_local(server).await?,
                )?)
            }
            .await,
        )
    }

    #[tool(
        description = "List or show stored versions, or export one exact artifact to a new absolute path inside MESHHARBOR_EXPORT_DIR."
    )]
    async fn engine_assets(
        &self,
        params: Parameters<engine_client::Request<engine_client::Assets>>,
    ) -> CallToolResult {
        engine_tool_result(
            async {
                engine_client::Client::configured()?
                    .assets(params.0.request, Some(&self.data_dir))
                    .await
            }
            .await,
        )
    }

    #[tool(description = "List 3D models or their versions, or make a saved version current.")]
    async fn engine_models(
        &self,
        params: Parameters<engine_client::Request<engine_client::Models>>,
    ) -> CallToolResult {
        engine_tool_result(
            async {
                engine_client::Client::configured()?
                    .models(params.0.request)
                    .await
            }
            .await,
        )
    }

    #[tool(
        description = "Simplify, clean up, remesh, unwrap UVs, optimize rendering, compress, run retopology, or preflight QuadriFlow on a saved mesh version."
    )]
    async fn engine_mesh(
        &self,
        params: Parameters<engine_client::Request<engine_client::Mesh>>,
    ) -> CallToolResult {
        engine_tool_result(
            async {
                engine_client::Client::configured()?
                    .mesh(params.0.request)
                    .await
            }
            .await,
        )
    }

    #[tool(
        description = "List waiting generation jobs or swap two adjacent jobs before they start."
    )]
    async fn engine_generation_queue(
        &self,
        params: Parameters<engine_client::Request<engine_client::GenerationQueue>>,
    ) -> CallToolResult {
        engine_tool_result(
            async {
                engine_client::Client::configured()?
                    .generation_queue(params.0.request)
                    .await
            }
            .await,
        )
    }

    #[tool(
        description = "List, inspect, cancel, or retry a recorded job. Confirm downloads with the owner before retrying runtime installations."
    )]
    async fn engine_jobs(
        &self,
        params: Parameters<engine_client::Request<engine_client::Jobs>>,
    ) -> CallToolResult {
        engine_tool_result(
            async {
                engine_client::Client::configured()?
                    .jobs(params.0.request)
                    .await
            }
            .await,
        )
    }

    #[tool(
        description = "Check readiness and manage runtime environments, verified backends, and model files. Ask the owner before installations and removals."
    )]
    async fn engine_runtime(
        &self,
        params: Parameters<engine_client::Request<engine_client::Runtime>>,
    ) -> CallToolResult {
        engine_tool_result(
            async {
                engine_client::Client::configured()?
                    .runtime(params.0.request)
                    .await
            }
            .await,
        )
    }

    #[tool(
        description = "Check a Hugging Face connection or manage browser device authorization for the mlx or cuda account scope. Never accept an access token as a tool argument."
    )]
    async fn engine_connections(
        &self,
        params: Parameters<engine_client::Request<engine_client::Connections>>,
    ) -> CallToolResult {
        engine_tool_result(
            async {
                engine_client::Client::configured()?
                    .connections(params.0.request)
                    .await
            }
            .await,
        )
    }

    #[tool(
        description = "Check Blender, transfer an exact model version, track a transfer, or download the bundled add-on."
    )]
    async fn engine_blender(
        &self,
        params: Parameters<engine_client::Request<engine_client::Blender>>,
    ) -> CallToolResult {
        engine_tool_result(
            async {
                engine_client::Client::configured()?
                    .blender(params.0.request, Some(&self.data_dir))
                    .await
            }
            .await,
        )
    }
}

#[tool_handler]
impl ServerHandler for McpServer {}

fn engine_tool_result(result: Result<Value, BoxError>) -> CallToolResult {
    match result {
        Ok(value) if value.is_object() => CallToolResult::structured(value),
        Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
        Err(error) => CallToolResult::structured_error(json!({"error": error.to_string()})),
    }
}

async fn read_bounded_asset(
    path: &std::path::Path,
    max_bytes: usize,
) -> Result<Vec<u8>, CoreError> {
    let read_limit = u64::try_from(max_bytes)
        .ok()
        .and_then(|limit| limit.checked_add(1))
        .ok_or_else(|| CoreError::InvalidInput("Asset size limit is too large.".into()))?;
    let file = fs::File::open(path).await?;
    let mut file = file.take(read_limit);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).await?;
    if bytes.len() > max_bytes {
        return Err(CoreError::InvalidInput(format!(
            "Asset exceeds the {} MiB input limit.",
            max_bytes / (1024 * 1024)
        )));
    }
    Ok(bytes)
}

struct JobService {
    state: AppState,
    _service_lock: data_dir_lock::DataDirLock,
    owner_lock: Option<data_dir_lock::DataDirLock>,
    owner: uuid::Uuid,
}

impl Drop for JobService {
    fn drop(&mut self) {
        drop(self.owner_lock.take());
        data_dir_lock::DataDirLock::remove_job_owner_file(
            self.state.core.store().root(),
            self.owner,
        );
    }
}

async fn open_job_service(
    root: &std::path::Path,
) -> Result<JobService, Box<dyn std::error::Error>> {
    use data_dir_lock::DataDirLock;
    use std::io::ErrorKind::WouldBlock;

    let service_lock = loop {
        match DataDirLock::try_acquire_for_recovery(root) {
            Ok(lock) => {
                let core = OperationCore::open(root)?;
                let recovered = core.store().recover_service_jobs(None)?;
                if !recovered.is_empty() {
                    tracing::info!(count = recovered.len(), "Recovered legacy interrupted jobs");
                }
                drop(lock);
            }
            Err(error) if error.kind() == WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        match DataDirLock::try_acquire_for_service(root) {
            Ok(lock) => break lock,
            Err(error) if error.kind() == WouldBlock => {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error.into()),
        }
    };
    let core = open_operation_core(root)?;
    for saved in core.store().active_job_owners()? {
        let owner = uuid::Uuid::parse_str(&saved)?;
        match DataDirLock::try_acquire_for_job_owner(root, owner) {
            Ok(lock) => {
                let recovered = core.store().recover_service_jobs(Some(&saved))?;
                if !recovered.is_empty() {
                    tracing::info!(count = recovered.len(), "Recovered interrupted jobs");
                }
                drop(lock);
                DataDirLock::remove_job_owner_file(root, owner);
            }
            Err(error) if error.kind() == WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
    }
    let owner = uuid::Uuid::new_v4();
    let owner_lock = DataDirLock::try_acquire_for_job_owner(root, owner)?;
    let scheduler = Arc::new(JobScheduler::owned(owner.to_string()));
    let detection_core = core.clone();
    match task::spawn_blocking(move || {
        let _runtime_lock = acquire_runtime_write_lock(detection_core.store().root())?;
        detection_core.detect_model_environment()
    })
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::warn!(%error, "Could not detect shared model tools at startup"),
        Err(error) => tracing::warn!(%error, "Shared model tool detection worker failed"),
    }
    Ok(JobService {
        state: AppState { core, scheduler },
        _service_lock: service_lock,
        owner_lock: Some(owner_lock),
        owner,
    })
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = terminate.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn stop_jobs(state: &AppState) {
    if let Err(error) = state.scheduler.stop(state.core.store()) {
        tracing::error!(%error, "Could not request job shutdown");
    }
}

fn open_operation_core(root: &std::path::Path) -> Result<OperationCore, CoreError> {
    let core = OperationCore::open(root)?;
    #[cfg(feature = "bundled-quadriflow")]
    let core = core.with_linked_quadriflow_worker()?;
    Ok(core)
}

async fn serve(
    root: &std::path::Path,
    listen: SocketAddr,
    shutdown: impl Future<Output = ()> + Send + 'static,
    report_startup: impl FnOnce(String),
) -> Result<(), Box<dyn std::error::Error>> {
    if !listen.ip().is_loopback() {
        return Err(
            "Refusing non-loopback bind; remote listening is not implemented in this build.".into(),
        );
    }
    let token = resolve_api_token(std::env::var("MESHHARBOR_API_TOKEN"))?;
    let listener = TcpListener::bind(listen).await?;
    let listen = listener.local_addr()?;
    let jobs = open_job_service(root).await?;
    let state = &jobs.state;
    let statuses = HuggingFaceStatuses::default();
    statuses.start(root);
    let app = api_router_with_statuses(state.clone(), token.value.clone(), statuses, listen);
    report_startup(format!(
        "MeshHarbor REST API listening at http://{listen}; {}",
        token_startup_log(&token)
    ));
    let shutdown_state = state.clone();
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.await;
            stop_jobs(&shutdown_state);
        })
        .await;
    stop_jobs(state);
    state.scheduler.drain().await;
    served?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "bundled-quadriflow")]
    {
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref()
            == Some(std::ffi::OsStr::new(
                meshharbor_core::LINKED_QUADRIFLOW_WORKER_ARG,
            ))
        {
            return quadriflow_worker::run(args);
        }
    }
    run()
}

#[tokio::main]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    #[cfg(windows)]
    if let Command::Serve {
        listen,
        background_id: Some(id),
    } = &cli.command
    {
        return service::windows::serve(&cli.data_dir, *listen, *id).await;
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("meshharbor=info".parse()?))
        .with_writer(std::io::stderr)
        .init();
    let root = cli.data_dir;
    let _store_lock = if matches!(cli.command, Command::Local { .. }) {
        Some(data_dir_lock::DataDirLock::try_acquire_for_service(&root)?)
    } else {
        None
    };
    match cli.command {
        Command::Serve { listen, .. } => {
            serve(&root, listen, shutdown_signal(), |message| {
                eprintln!("{message}")
            })
            .await?;
        }
        Command::Service { command } => service::run(command, &root)?,
        Command::Uninstall {
            confirm,
            keep_binary,
        } => uninstall::run(&root, confirm, keep_binary)?,
        Command::Mcp => {
            let server = McpServer::new(root);
            let service = server.serve(rmcp::transport::stdio()).await?;
            let cancellation = service.cancellation_token();
            let waiting = service.waiting();
            tokio::pin!(waiting);
            let closed = tokio::select! {
                result = &mut waiting => result,
                _ = shutdown_signal() => {
                    cancellation.cancel();
                    waiting.await
                }
            };
            closed?;
        }
        Command::Engine { server, command } => {
            let result = command
                .run(server)
                .await
                .map_err(|error| -> Box<dyn std::error::Error> { error })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::InstallBlenderAddon { server } => {
            let result = blender_bridge::install_local(server).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Local { command } => match command {
            LocalCommand::Import { file } => {
                let bytes = read_bounded_asset(&file, MAX_ASSET_BYTES).await?;
                let name = file
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("asset");
                let asset = OperationCore::open(&root)?.import_asset(name, &bytes)?;
                println!("{}", serde_json::to_string_pretty(&asset)?);
            }
            LocalCommand::Inspect { file } => {
                let bytes = read_bounded_asset(&file, MAX_ASSET_BYTES).await?;
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
            LocalCommand::Assets { limit } => {
                let assets = OperationCore::open(&root)?.store().list_assets(limit)?;
                println!("{}", serde_json::to_string_pretty(&assets)?);
            }
            LocalCommand::Simplify {
                asset_id,
                target_triangles,
                max_error,
                output,
            } => {
                preflight_cli_output_paths(output.as_deref(), None)?;
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
                    write_cli_output(&path, &bytes)?;
                }
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
            LocalCommand::RetopologyPreflight {
                asset_id,
                target_quads,
            } => {
                let core = OperationCore::open(&root)?;
                let result =
                    core.preflight_retopology_asset(&asset_id, retopology_options(target_quads))?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
            LocalCommand::ProcessMesh {
                asset_id,
                operation,
                target_quads,
                resolution,
                voxel_resolution,
                padding,
                overdraw_threshold,
                for_retopology,
                output,
                quad_output,
            } => {
                validate_process_mesh_cli_options(
                    &operation,
                    target_quads,
                    quad_output.is_some(),
                    resolution,
                    voxel_resolution,
                    padding,
                    overdraw_threshold,
                )?;
                if for_retopology && operation != "mesh.cleanup" {
                    return Err("--for-retopology requires mesh.cleanup.".into());
                }
                preflight_cli_output_paths(output.as_deref(), quad_output.as_deref())?;
                let core = open_operation_core(&root)?;
                let mut parameters = process_mesh_cli_parameters(
                    &operation,
                    target_quads,
                    resolution,
                    voxel_resolution,
                    padding,
                    overdraw_threshold,
                );
                if for_retopology {
                    parameters["for_retopology"] = json!(true);
                }
                let result = core.process_mesh_asset(&asset_id, &operation, parameters)?;
                if let Some(path) = output {
                    let artifact = result
                        .asset
                        .artifacts
                        .iter()
                        .find(|artifact| artifact.kind == "derived")
                        .ok_or("Mesh processing output artifact was not found.")?;
                    let bytes = core.store().read_artifact(&artifact.id)?.bytes;
                    write_cli_output(&path, &bytes)?;
                }
                if let Some(path) = quad_output {
                    let artifact = result
                        .asset
                        .artifacts
                        .iter()
                        .find(|artifact| artifact.kind == "quad_topology")
                        .ok_or("Quad retopology OBJ artifact was not found.")?;
                    let bytes = core.store().read_artifact(&artifact.id)?.bytes;
                    write_cli_output(&path, &bytes)?;
                }
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
            LocalCommand::Generate {
                asset_id,
                prompt,
                backend_id,
                seed,
                resolution,
                texture_size,
                target_faces,
                target_vertices,
                tuning,
                view_input,
                output,
            } => {
                preflight_cli_output_paths(output.as_deref(), None)?;
                let _runtime_lock = acquire_generation_runtime_lock(&root)?;
                let core = OperationCore::open(&root)?;
                let options = GenerationOptions {
                    backend_id,
                    seed,
                    resolution,
                    texture_size,
                    target_faces,
                    target_vertices,
                    tuning: tuning.unwrap_or_default(),
                    view_input,
                };
                let result = match (asset_id, prompt) {
                    (Some(asset_id), None) => core.generate_image(&asset_id, options)?,
                    (None, Some(prompt)) => {
                        core.generate_model(&GenerationInput::Text { prompt }, options)?
                    }
                    _ => unreachable!("Clap requires one generation input"),
                };
                if let Some(path) = output {
                    let artifact = result
                        .asset
                        .artifacts
                        .iter()
                        .find(|artifact| artifact.kind == "derived")
                        .ok_or("Generation output artifact was not found.")?;
                    let bytes = core.store().read_artifact(&artifact.id)?.bytes;
                    write_cli_output(&path, &bytes)?;
                }
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
            LocalCommand::Doctor => println!(
                "{}",
                serde_json::to_string_pretty(&open_operation_core(&root)?.runtime_doctor())?
            ),
        },
        Command::RecoverJobs { .. } => {
            let _data_dir_lock = data_dir_lock::DataDirLock::try_acquire_for_recovery(&root)?;
            let core = OperationCore::open(&root)?;
            let recovered = core.store().recover_interrupted_jobs()?;
            eprintln!(
                "Marked {} interrupted job(s) as failed. Existing assets and download caches were left untouched; inspect them before retrying.",
                recovered.len()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
fn api_router(state: AppState, token: String) -> Router {
    api_router_with_statuses(
        state,
        token,
        HuggingFaceStatuses::default(),
        SocketAddr::from(([127, 0, 0, 1], 7432)),
    )
}

fn api_router_with_statuses(
    state: AppState,
    token: String,
    statuses: HuggingFaceStatuses,
    listen: SocketAddr,
) -> Router {
    Router::new()
        .merge(blender_bridge::routes(listen))
        .merge(openapi::routes())
        .route("/api/v1/health", get(api_health))
        .route("/api/v1/runtime", get(api_runtime))
        .route(
            "/api/v1/runtime/environment/setup",
            post(api_start_model_environment).layer(DefaultBodyLimit::max(1024)),
        )
        .route(
            "/api/v1/connections/huggingface/{scope}/sessions",
            post(browser_login::start),
        )
        .route(
            "/api/v1/connections/huggingface/{scope}/sessions/{session}",
            get(browser_login::status).delete(browser_login::cancel),
        )
        .route(
            "/api/v1/connections/huggingface/{scope}",
            post(api_huggingface_connection)
                .delete(api_disconnect_huggingface)
                .layer(DefaultBodyLimit::max(2048)),
        )
        .route(
            "/api/v1/connections/huggingface/{scope}/check",
            post(api_check_huggingface_connection),
        )
        .route(
            "/api/v1/connections/huggingface/{scope}/model-access",
            post(api_model_access),
        )
        .route(
            "/api/v1/runtime/{backend}/model-files",
            delete(api_remove_model_files).layer(DefaultBodyLimit::max(1024)),
        )
        .route(
            "/api/v1/runtime/{backend}/install",
            post(api_start_catalog_install).layer(DefaultBodyLimit::max(1024)),
        )
        .route("/api/v1/runtime/{backend}/jobs", get(api_runtime_jobs))
        .route(
            "/api/v1/runtime/quadriflow",
            get(api_quadriflow_backend_status),
        )
        .route(
            "/api/v1/runtime/quadriflow/install",
            post(api_install_quadriflow_backend),
        )
        .route("/api/v1/operations", get(api_operations))
        .route(
            "/api/v1/assets",
            get(api_list_asset_page)
                .post(api_import_asset)
                .layer(DefaultBodyLimit::max(MAX_ASSET_BYTES)),
        )
        .route("/api/v1/models", get(api_list_model_page))
        .route(
            "/api/v1/models/{model_id}/versions",
            get(api_model_versions),
        )
        .route(
            "/api/v1/models/{model_id}/current",
            axum::routing::put(api_restore_model_version),
        )
        .route("/api/v1/assets/{asset_id}", get(api_get_asset))
        .route("/api/v1/assets/{asset_id}/simplify", post(api_simplify))
        .route("/api/v1/assets/{asset_id}/cleanup", post(api_cleanup))
        .route(
            "/api/v1/assets/{asset_id}/voxel-remesh",
            post(api_voxel_remesh),
        )
        .route("/api/v1/assets/{asset_id}/unwrap-uv", post(api_unwrap_uv))
        .route(
            "/api/v1/assets/{asset_id}/optimize-rendering",
            post(api_optimize_rendering),
        )
        .route("/api/v1/assets/{asset_id}/compress", post(api_compress))
        .route("/api/v1/assets/{asset_id}/retopology", post(api_retopology))
        .route(
            "/api/v1/assets/{asset_id}/retopology-lato2",
            post(api_lato2_retopology),
        )
        .route(
            "/api/v1/assets/{asset_id}/retopology-preflight",
            post(api_retopology_preflight).layer(DefaultBodyLimit::max(1024)),
        )
        .route("/api/v1/generation", post(api_generate_model))
        .route("/api/v1/jobs", get(api_list_jobs))
        .route(
            "/api/v1/generation/queue",
            get(api_generation_queue)
                .post(api_swap_generation_queue)
                .layer(DefaultBodyLimit::max(1024)),
        )
        .route("/api/v1/jobs/{job_id}", get(api_get_job))
        .route("/api/v1/jobs/{job_id}/steps", get(api_job_steps))
        .route("/api/v1/jobs/{job_id}/cancel", post(api_cancel_job))
        .route("/api/v1/jobs/{job_id}/retry", post(api_retry_job))
        .route("/api/v1/artifacts/{artifact_id}", get(api_read_artifact))
        .fallback(api_studio_fallback)
        .layer(axum::Extension(
            browser_login::BrowserLoginSessions::default(),
        ))
        .layer(Extension(statuses))
        .layer(middleware::from_fn_with_state(token, authenticate))
        .with_state(state)
}

async fn authenticate(State(token): State<String>, request: Request, next: Next) -> Response {
    const DASHBOARD_MARKER: &str = "local-ui-v1";
    let path = request.uri().path();
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    let host_ok = host.is_some_and(is_loopback_authority);
    if path.starts_with("/api/v1/blender/client/") {
        let headers = request.headers();
        if host_ok
            && request.method() == Method::POST
            && ![
                header::ORIGIN,
                header::REFERER,
                header::HeaderName::from_static("sec-fetch-site"),
            ]
            .iter()
            .any(|name| headers.contains_key(name))
            && headers
                .get("x-meshharbor-blender")
                .is_some_and(|value| value == "local-addon-v1")
        {
            return next.run(request).await;
        }
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"A native loopback Blender client is required."})),
        )
            .into_response();
    }
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
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let bearer_authorized = authorization
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|provided| constant_time_equal(provided.as_bytes(), token.as_bytes()));
    let origin_required = is_api_path(request.uri().path())
        && request.method() != Method::GET
        && request.method() != Method::HEAD
        && authorization.is_none();
    let origin_present = request.headers().contains_key(header::ORIGIN);
    if !host_ok || !origin_ok || (origin_required && !origin_present) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"API requests require a loopback Host; tokenless state-changing requests require an exact same-origin Origin."})),
        )
            .into_response();
    }
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
    let dashboard_only = path == "/api/v1/blender/install"
        || (request.method() == Method::POST
            && path.starts_with("/api/v1/connections/huggingface/")
            && path.split('/').count() == 6);
    if dashboard_authorized || (authorized && !dashboard_only) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": if dashboard_only {
                "A same-origin Studio request is required."
            } else {
                "A valid bearer token or same-origin Studio request is required."
            }})),
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
async fn api_runtime(
    State(state): State<AppState>,
    Extension(statuses): Extension<HuggingFaceStatuses>,
) -> impl IntoResponse {
    let mut runtime = state.core.runtime_doctor();
    statuses.decorate(&mut runtime);
    ([(header::CACHE_CONTROL, "no-store")], Json(runtime))
}

async fn api_runtime_jobs(
    State(state): State<AppState>,
    Path(backend): Path<String>,
) -> Result<Json<Vec<meshharbor_core::Job>>, ApiError> {
    Ok(Json(state.core.store().list_runtime_jobs(&backend)?))
}

fn retopology_options(target_quads: Option<u32>) -> meshharbor_core::QuadRetopologyOptions {
    meshharbor_core::QuadRetopologyOptions { target_quads }
}

async fn run_retopology_preflight(
    core: OperationCore,
    scheduler: Arc<JobScheduler>,
    asset_id: String,
    options: meshharbor_core::QuadRetopologyOptions,
) -> Result<meshharbor_core::QuadRetopologyPreflight, CoreError> {
    let permit = scheduler.try_acquire_owned().map_err(|_| {
        CoreError::Conflict(
            "MeshHarbor is busy. Retry the retopology check after the current operation.".into(),
        )
    })?;
    task::spawn_blocking(move || {
        let _permit = permit;
        core.preflight_retopology_asset(&asset_id, options)
    })
    .await
    .map_err(|_| CoreError::OperationUnavailable("Retopology preflight worker failed.".into()))?
}

async fn api_retopology_preflight(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
    Json(options): Json<meshharbor_core::QuadRetopologyOptions>,
) -> Result<Json<meshharbor_core::QuadRetopologyPreflight>, ApiError> {
    Ok(Json(
        run_retopology_preflight(state.core, state.scheduler, asset_id, options).await?,
    ))
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HuggingFaceConnectionRequest {
    token: Option<String>,
}

async fn api_huggingface_connection(
    State(state): State<AppState>,
    Extension(statuses): Extension<HuggingFaceStatuses>,
    Path(scope): Path<actions::AccountScope>,
    payload: Result<Json<HuggingFaceConnectionRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(payload) = payload
        .map_err(|_| CoreError::InvalidInput("Invalid Hugging Face connection request.".into()))?;
    check_huggingface_connection(state, statuses, scope.into(), payload.token).await
}

async fn api_check_huggingface_connection(
    State(state): State<AppState>,
    Extension(statuses): Extension<HuggingFaceStatuses>,
    Path(scope): Path<actions::AccountScope>,
) -> Result<impl IntoResponse, ApiError> {
    check_huggingface_connection(state, statuses, scope.into(), None).await
}

async fn check_huggingface_connection(
    state: AppState,
    statuses: HuggingFaceStatuses,
    profile: Pixal3dProfile,
    token: Option<String>,
) -> Result<impl IntoResponse, ApiError> {
    let access = task::spawn_blocking(move || {
        let root = state.core.store().root();
        let _runtime_lock = if token.is_some() {
            acquire_runtime_write_lock(root)?
        } else {
            acquire_generation_runtime_lock(root)?
        };
        let result = huggingface_status::check_connection(root, profile, token.as_deref());
        if token.is_none()
            || matches!(
                result,
                Ok(meshharbor_core::HuggingFaceConnection::Connected)
            )
        {
            statuses.record_account(root, profile, result.as_ref().ok());
        }
        result
    })
    .await
    .map_err(|_| {
        CoreError::OperationUnavailable("Hugging Face connection worker failed.".into())
    })??;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(access)))
}

async fn api_model_access(
    State(state): State<AppState>,
    Extension(statuses): Extension<HuggingFaceStatuses>,
    Path(scope): Path<actions::AccountScope>,
) -> Result<impl IntoResponse, ApiError> {
    let profile = Pixal3dProfile::from(scope);
    let access = task::spawn_blocking(move || {
        let root = state.core.store().root();
        let _runtime_lock = acquire_generation_runtime_lock(root)?;
        let result = huggingface_status::check_access(root, profile);
        statuses.record_access(profile, result.as_ref().ok());
        result
    })
    .await
    .map_err(|_| CoreError::OperationUnavailable("Model access worker failed.".into()))??;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(access)))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RuntimeRemovalConfirmation {
    confirmed: bool,
}

async fn api_disconnect_huggingface(
    State(state): State<AppState>,
    Extension(statuses): Extension<HuggingFaceStatuses>,
    Path(scope): Path<actions::AccountScope>,
    Json(confirmation): Json<RuntimeRemovalConfirmation>,
) -> Result<Json<RuntimeDoctor>, ApiError> {
    let profile = Pixal3dProfile::from(scope);
    remove_runtime_files(state.core, confirmation, statuses.clone(), move |core| {
        profile.disconnect_huggingface(core.store().root())?;
        statuses.disconnected(profile);
        Ok(())
    })
    .await
}

async fn api_remove_model_files(
    State(state): State<AppState>,
    Extension(statuses): Extension<HuggingFaceStatuses>,
    Path(backend): Path<String>,
    Json(confirmation): Json<RuntimeRemovalConfirmation>,
) -> Result<Json<RuntimeDoctor>, ApiError> {
    remove_runtime_files(state.core, confirmation, statuses, move |core| {
        core.remove_model_files(&backend)
    })
    .await
}

async fn remove_runtime_files(
    core: OperationCore,
    confirmation: RuntimeRemovalConfirmation,
    statuses: HuggingFaceStatuses,
    remove: impl FnOnce(&OperationCore) -> Result<(), CoreError> + Send + 'static,
) -> Result<Json<RuntimeDoctor>, ApiError> {
    if !confirmation.confirmed {
        return Err(CoreError::InvalidInput(
            "Confirm removal before changing local runtime files.".into(),
        )
        .into());
    }
    let runtime = task::spawn_blocking(move || {
        let root = core.store().root();
        let _runtime_lock = acquire_runtime_write_lock(root)?;
        remove(&core)?;
        let mut runtime = core.runtime_doctor();
        statuses.decorate(&mut runtime);
        Ok::<_, CoreError>(runtime)
    })
    .await
    .map_err(|_| {
        CoreError::OperationUnavailable("Local runtime file removal worker failed.".into())
    })??;
    Ok(Json(runtime))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EnvironmentSetupConfirmation {
    confirmed: bool,
}

async fn api_start_model_environment(
    State(state): State<AppState>,
    Json(confirmation): Json<EnvironmentSetupConfirmation>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    if !confirmation.confirmed {
        return Err(CoreError::InvalidInput(
            "Confirm before setting up the shared environment.".into(),
        )
        .into());
    }
    let job = start_runtime_job(
        state.core,
        state.scheduler,
        RuntimeJobKind::EnvironmentSetup,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn api_start_catalog_install(
    State(state): State<AppState>,
    Path(backend): Path<String>,
    Json(confirmation): Json<actions::InstallBackend>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    if !confirmation.confirmed {
        return Err(CoreError::InvalidInput(
            "Review the model terms and confirm installation first.".into(),
        )
        .into());
    }
    let kind = match confirmation.package {
        actions::RuntimePackage::Base => state
            .core
            .catalog_install_job(&backend, confirmation.confirm_gated_access)?,
        actions::RuntimePackage::Multiview if backend == "pixal3d-cuda" => {
            RuntimeJobKind::CudaInstallMultiview
        }
        actions::RuntimePackage::Multiview => {
            return Err(
                CoreError::InvalidInput("This backend has no multiview package.".into()).into(),
            );
        }
    };
    let job = start_runtime_job(state.core, state.scheduler, kind).await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}
async fn api_operations(
    State(state): State<AppState>,
) -> Json<Vec<meshharbor_core::OperationDescriptor>> {
    Json(state.core.list_operations())
}
async fn api_list_asset_page(
    State(state): State<AppState>,
    Query(query): Query<AssetPageQuery>,
) -> Result<Json<meshharbor_core::AssetPage>, ApiError> {
    Ok(Json(state.core.store().list_assets_page(
        query.limit.unwrap_or(50),
        query.cursor.as_deref(),
        query.q.as_deref(),
        query.kind,
        query.derived,
    )?))
}

async fn api_list_model_page(
    State(state): State<AppState>,
    Query(query): Query<ModelPageQuery>,
) -> Result<Json<meshharbor_core::AssetPage>, ApiError> {
    Ok(Json(state.core.store().list_models_page(
        query.limit.unwrap_or(50),
        query.cursor.as_deref(),
        query.q.as_deref(),
        query.generated,
    )?))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ModelPageQuery {
    limit: Option<usize>,
    cursor: Option<String>,
    q: Option<String>,
    generated: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AssetPageQuery {
    limit: Option<usize>,
    cursor: Option<String>,
    q: Option<String>,
    kind: Option<meshharbor_core::AssetKind>,
    derived: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ModelVersionsQuery {
    limit: Option<usize>,
    before: Option<u64>,
}

async fn api_model_versions(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
    Query(query): Query<ModelVersionsQuery>,
) -> Result<Json<meshharbor_core::ModelVersionPage>, ApiError> {
    Ok(Json(state.core.store().list_model_versions(
        &model_id,
        query.limit.unwrap_or(20),
        query.before,
    )?))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RestoreModelVersion {
    asset_id: String,
}

async fn api_restore_model_version(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
    Json(body): Json<RestoreModelVersion>,
) -> Result<Json<meshharbor_core::Asset>, ApiError> {
    Ok(Json(
        state
            .core
            .store()
            .restore_model_version(&model_id, &body.asset_id)?,
    ))
}

async fn api_import_asset(
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    body: Bytes,
) -> Result<Json<meshharbor_core::Asset>, ApiError> {
    if body.len() > MAX_ASSET_BYTES {
        return Err(ApiError(CoreError::InvalidInput(format!(
            "Asset exceeds the {} MiB request limit.",
            MAX_ASSET_BYTES / (1024 * 1024)
        ))));
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
    Json(body): Json<actions::SimplifyMesh>,
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

macro_rules! mesh_endpoint {
    ($name:ident, $options:ty, $operation:expr) => {
        async fn $name(
            State(state): State<AppState>,
            Path(asset_id): Path<String>,
            Json(body): Json<$options>,
        ) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
            let parameters = serde_json::to_value(body).map_err(CoreError::from)?;
            let job = start_mesh_operation_job(
                state.core,
                state.scheduler,
                &asset_id,
                $operation,
                parameters,
            )
            .await?;
            Ok((StatusCode::ACCEPTED, Json(job)))
        }
    };
}

mesh_endpoint!(api_cleanup, actions::CleanupMesh, "mesh.cleanup");
mesh_endpoint!(api_voxel_remesh, actions::VoxelRemesh, "mesh.voxel_remesh");
mesh_endpoint!(api_unwrap_uv, actions::UnwrapUv, "uv.unwrap");
mesh_endpoint!(
    api_optimize_rendering,
    actions::OptimizeRendering,
    "render.optimize"
);
mesh_endpoint!(api_compress, actions::CompressMesh, "mesh.compress");
mesh_endpoint!(
    api_retopology,
    meshharbor_core::QuadRetopologyOptions,
    "mesh.retopology"
);
mesh_endpoint!(
    api_lato2_retopology,
    actions::RetopologizeLato2,
    meshharbor_core::LATO2_OPERATION
);

async fn api_generate_model(
    State(state): State<AppState>,
    Json(body): Json<actions::GenerateModel>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    let job =
        start_generation_job(state.core, state.scheduler, body.input, body.parameters).await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn api_retry_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    Json(confirmation): Json<actions::RetryJob>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    let saved = read_job_record(&state.core, &job_id)?;
    if !matches!(
        saved.state,
        meshharbor_core::JobState::Failed | meshharbor_core::JobState::Cancelled
    ) {
        return Err(
            CoreError::Conflict("Only failed or stopped jobs can be retried.".into()).into(),
        );
    }
    let kind = match saved.operation.as_str() {
        "runtime.environment.setup" => Some(RuntimeJobKind::EnvironmentSetup),
        "runtime.pixal3d.cuda_install_multiview" => Some(RuntimeJobKind::CudaInstallMultiview),
        "runtime.models.install" => {
            let backend = saved.parameters["backend_id"].as_str().ok_or_else(|| {
                CoreError::Conflict("This record has no saved runtime backend.".into())
            })?;
            Some(state.core.catalog_install_job(backend, true)?)
        }
        "runtime.geosam2.install_backend" | "runtime.geosam2.download_weights" => {
            return Err(CoreError::Conflict(
                "GeoSAM2 is now installed in one job. Use Install model in Settings → Models."
                    .into(),
            )
            .into());
        }
        _ => None,
    };
    let job = if let Some(kind) = kind {
        if kind.downloads_weights() && !confirmation.confirm_download {
            return Err(CoreError::InvalidInput(
                "Confirm the model download before retrying this installation.".into(),
            )
            .into());
        }
        start_runtime_job(state.core, state.scheduler, kind).await?
    } else {
        if saved.parameters["input_mode"] == "text" {
            return Err(CoreError::Conflict(
                "Text prompts are not saved. Enter the prompt again in New model.".into(),
            )
            .into());
        }
        let asset_id = saved
            .input_asset_id
            .ok_or_else(|| CoreError::Conflict("This record has no saved input asset.".into()))?;
        let mut parameters = saved.parameters;
        if let Some(parameters) = parameters.as_object_mut() {
            parameters.remove("input_mode");
        }
        match saved.operation.as_str() {
            MODEL_GENERATION_OPERATION => {
                let options = serde_json::from_value(parameters).map_err(CoreError::from)?;
                start_generation_job(
                    state.core,
                    state.scheduler,
                    GenerationInput::Image { asset_id },
                    options,
                )
                .await?
            }
            "mesh.simplify" => {
                let options: actions::SimplifyMesh =
                    serde_json::from_value(parameters).map_err(CoreError::from)?;
                start_simplify_job(
                    state.core,
                    state.scheduler,
                    &asset_id,
                    options.target_triangles,
                    options.max_error,
                )
                .await?
            }
            operation @ ("mesh.cleanup"
            | "mesh.voxel_remesh"
            | "uv.unwrap"
            | "render.optimize"
            | "mesh.compress"
            | "mesh.retopology"
            | meshharbor_core::LATO2_OPERATION) => {
                start_mesh_operation_job(
                    state.core,
                    state.scheduler,
                    &asset_id,
                    operation,
                    parameters,
                )
                .await?
            }
            _ => {
                return Err(CoreError::OperationUnavailable(
                    "This recorded operation is no longer available for retry.".into(),
                )
                .into());
            }
        }
    };
    Ok((StatusCode::ACCEPTED, Json(job)))
}

fn read_job_history(
    core: &OperationCore,
    limit: usize,
) -> Result<Vec<meshharbor_core::Job>, CoreError> {
    core.store().list_jobs(limit.clamp(1, 200))
}

fn read_job_record(core: &OperationCore, job_id: &str) -> Result<meshharbor_core::Job, CoreError> {
    core.store()
        .get_job(job_id)?
        .ok_or_else(|| CoreError::NotFound(format!("Job '{job_id}' was not found.")))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct JobHistoryQuery {
    limit: Option<usize>,
    include_active: Option<bool>,
    before: Option<String>,
}

async fn api_list_jobs(
    State(state): State<AppState>,
    Query(query): Query<JobHistoryQuery>,
) -> Result<Json<Vec<meshharbor_core::Job>>, ApiError> {
    let limit = query.limit.unwrap_or(50);
    let jobs = if query.include_active.unwrap_or(false) || query.before.is_some() {
        state
            .core
            .store()
            .list_dashboard_jobs(limit, query.before.as_deref())?
    } else {
        read_job_history(&state.core, limit)?
    };
    Ok(Json(jobs))
}

async fn api_get_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<meshharbor_core::Job>, ApiError> {
    Ok(Json(read_job_record(&state.core, &job_id)?))
}

async fn api_job_steps(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(state.core.store().job_steps(&job_id)?),
    ))
}

#[derive(Deserialize, JsonSchema)]
struct CancelJobQuery {
    #[serde(default)]
    queued_only: bool,
}

async fn api_cancel_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    Query(query): Query<CancelJobQuery>,
) -> Result<Json<meshharbor_core::Job>, ApiError> {
    Ok(Json(cancel_job(
        &state.core,
        &state.scheduler,
        &job_id,
        query.queued_only,
    )?))
}

fn cancel_job(
    core: &OperationCore,
    scheduler: &JobScheduler,
    job_id: &str,
    queued_only: bool,
) -> Result<meshharbor_core::Job, CoreError> {
    let job = core.store().request_job_cancellation(job_id, queued_only)?;
    if job.state == meshharbor_core::JobState::Cancelled {
        scheduler.remove(job_id);
    }
    Ok(job)
}

#[derive(Deserialize, JsonSchema)]
struct GenerationQueueSwap {
    job_id: String,
    swap_with: String,
}

async fn api_generation_queue(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.scheduler.generation_queue())
}

async fn api_swap_generation_queue(
    State(state): State<AppState>,
    Json(payload): Json<GenerationQueueSwap>,
) -> Result<Json<Vec<String>>, ApiError> {
    Ok(Json(
        state
            .scheduler
            .swap_generation(&payload.job_id, &payload.swap_with)?,
    ))
}

fn finish_job(
    core: &OperationCore,
    job_id: &str,
    result: Result<Result<Value, CoreError>, task::JoinError>,
) {
    let result = result.unwrap_or_else(|error| {
        Err(CoreError::OperationUnavailable(format!(
            "Job worker failed: {error}"
        )))
    });
    if let Err(error) = core.store().finish_job(job_id, result) {
        eprintln!("Could not finalize job {job_id}: {error}");
    }
}

async fn start_simplify_job(
    core: OperationCore,
    scheduler: Arc<JobScheduler>,
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
    let (job, reserved) = scheduler.submit(
        core.store(),
        "mesh.simplify",
        Some(asset_id),
        &json!({"target_triangles":target_triangles,"max_error":max_error}),
    )?;
    let job_id = job.id.clone();
    let worker_asset_id = asset_id.to_owned();
    tokio::spawn(async move {
        let Some(permit) = scheduler.start(core.store(), &job_id, reserved).await else {
            return;
        };
        let worker_core = core.clone();
        let worker_job_id = job_id.clone();
        let result = task::spawn_blocking(move || {
            let result = worker_core.simplify_asset_for_job(
                &worker_asset_id,
                target_triangles,
                max_error,
                &worker_job_id,
            )?;
            Ok(json!({"asset_id": result.asset.id, "report": result.report}))
        })
        .await;
        finish_job(&core, &job_id, result);
        drop(permit);
    });
    Ok(job)
}

async fn start_mesh_operation_job(
    core: OperationCore,
    scheduler: Arc<JobScheduler>,
    asset_id: &str,
    operation: &str,
    parameters: Value,
) -> Result<meshharbor_core::Job, CoreError> {
    if operation == meshharbor_core::LATO2_OPERATION {
        core.check_lato2_mesh_backend(&parameters)?;
    }
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
    let (job, reserved) = scheduler.submit(core.store(), operation, Some(asset_id), &parameters)?;
    let job_id = job.id.clone();
    let worker_operation = operation.to_string();
    let worker_asset_id = asset_id.to_owned();
    tokio::spawn(async move {
        let Some(permit) = scheduler.start(core.store(), &job_id, reserved).await else {
            return;
        };
        let worker_core = core.clone();
        let worker_parameters = parameters;
        let worker_job_id = job_id.clone();
        let result = task::spawn_blocking(move || {
            let _runtime_lock = (worker_operation == meshharbor_core::LATO2_OPERATION)
                .then(|| acquire_generation_runtime_lock(worker_core.store().root()))
                .transpose()?;
            let result = worker_core.process_mesh_asset_for_job(
                &worker_asset_id,
                &worker_operation,
                worker_parameters,
                &worker_job_id,
            )?;
            Ok(json!({"asset_id": result.asset.id, "report": result.report}))
        })
        .await;
        finish_job(&core, &job_id, result);
        drop(permit);
    });
    Ok(job)
}

async fn start_generation_job(
    core: OperationCore,
    scheduler: Arc<JobScheduler>,
    input: GenerationInput,
    options: GenerationOptions,
) -> Result<meshharbor_core::Job, CoreError> {
    core.check_model_generation(&input, &options)?;

    let mut parameters = serde_json::to_value(&options)?;
    if meshharbor_core::is_lato2_backend(&options.backend_id)
        && let Some(parameters) = parameters.as_object_mut()
    {
        for field in ["target_faces", "resolution", "texture_size"] {
            parameters.remove(field);
        }
    }
    parameters["input_mode"] = json!(input.mode());
    let (job, reserved) = scheduler.submit(
        core.store(),
        MODEL_GENERATION_OPERATION,
        input.asset_id(),
        &parameters,
    )?;
    let job_id = job.id.clone();
    tokio::spawn(async move {
        let Some(permit) = scheduler.start(core.store(), &job_id, reserved).await else {
            return;
        };

        let worker_core = core.clone();
        let worker_data_dir = core.store().root().to_path_buf();
        let worker_job_id = job_id.clone();
        let result = task::spawn_blocking(move || {
            worker_core
                .store()
                .update_running_job_stage(&worker_job_id, "check_runtime")?;
            let _runtime_lock = acquire_generation_runtime_lock(&worker_data_dir)?;
            let result = worker_core.generate_model_for_job(&input, options, &worker_job_id)?;
            let mut payload = json!({
                "asset_id": result.asset.id,
                "backend_id": result.backend_id,
                "seed": result.seed,
                "elapsed_ms": result.elapsed_ms,
            });
            if let Some(target) = result.target_faces {
                payload["target_faces"] = json!(target);
            }
            if let Some(target) = result.target_vertices {
                payload["target_vertices"] = json!(target);
            }
            if let Some(resolution) = result.resolution {
                payload["resolution"] = json!(resolution);
            }
            if let Some(size) = result.texture_size {
                payload["texture_size"] = json!(size);
            }
            Ok(payload)
        })
        .await;
        finish_job(&core, &job_id, result);
        drop(permit);
    });
    Ok(job)
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
    scheduler: Arc<JobScheduler>,
    kind: RuntimeJobKind,
) -> Result<meshharbor_core::Job, CoreError> {
    core.validate_runtime_job(kind)?;
    let (job, reserved) = scheduler.submit(
        core.store(),
        kind.operation(),
        None,
        &json!({"backend_id":kind.backend_id()}),
    )?;
    let job_id = job.id.clone();
    tokio::spawn(async move {
        let Some(permit) = scheduler.start(core.store(), &job_id, reserved).await else {
            return;
        };

        let worker_core = core.clone();
        let worker_data_dir = core.store().root().to_path_buf();
        let worker_job_id = job_id.clone();
        let result = task::spawn_blocking(move || {
            worker_core
                .store()
                .update_running_job_stage(&worker_job_id, "check_runtime")?;
            let _runtime_lock = acquire_runtime_write_lock(&worker_data_dir)?;
            worker_core.run_runtime_job(kind, &worker_job_id)
        })
        .await;

        finish_job(&core, &job_id, result);
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

fn canonical_cli_output_path(path: &std::path::Path) -> Result<PathBuf, CoreError> {
    let file_name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| CoreError::InvalidInput("CLI output path must name a file.".into()))?;
    let requested_parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let absolute_parent = if requested_parent.is_absolute() {
        requested_parent.to_path_buf()
    } else {
        std::env::current_dir()?.join(requested_parent)
    };
    let canonical_parent = std::fs::canonicalize(absolute_parent)?;
    if !canonical_parent.is_dir() {
        return Err(CoreError::InvalidInput(
            "CLI output parent must be an existing directory.".into(),
        ));
    }
    Ok(canonical_parent.join(file_name))
}

#[cfg(windows)]
fn cli_output_destinations_alias(
    first: &std::path::Path,
    second: &std::path::Path,
) -> Result<bool, CoreError> {
    use std::os::windows::ffi::OsStrExt;

    let first: Vec<u16> = first.as_os_str().encode_wide().collect();
    let second: Vec<u16> = second.as_os_str().encode_wide().collect();
    let first_len = i32::try_from(first.len()).map_err(|_| {
        CoreError::InvalidInput("CLI output path is too long to compare safely.".into())
    })?;
    let second_len = i32::try_from(second.len()).map_err(|_| {
        CoreError::InvalidInput("CLI output path is too long to compare safely.".into())
    })?;

    // CompareStringOrdinal compares Windows file names without linguistic rules.
    // Its lengths exclude terminating NULs, and 1 requests case-insensitive comparison.
    let result = unsafe {
        windows_sys::Win32::Globalization::CompareStringOrdinal(
            first.as_ptr(),
            first_len,
            second.as_ptr(),
            second_len,
            1,
        )
    };
    match result {
        1 | 3 => Ok(false),
        2 => Ok(true),
        _ => Err(CoreError::Io(std::io::Error::last_os_error())),
    }
}

#[cfg(not(windows))]
fn cli_output_destinations_alias(
    first: &std::path::Path,
    second: &std::path::Path,
) -> Result<bool, CoreError> {
    Ok(first == second)
}

fn preflight_cli_output_paths(
    output_path: Option<&std::path::Path>,
    quad_output_path: Option<&std::path::Path>,
) -> Result<(), CoreError> {
    let output_destination = output_path.map(canonical_cli_output_path).transpose()?;
    let quad_destination = quad_output_path
        .map(canonical_cli_output_path)
        .transpose()?;
    let destinations_alias = match (&output_destination, &quad_destination) {
        (Some(output), Some(quad)) => cli_output_destinations_alias(output, quad)?,
        _ => false,
    };
    if destinations_alias {
        return Err(CoreError::InvalidInput(
            "--output and --quad-output must name different destinations.".into(),
        ));
    }

    for destination in output_destination.iter().chain(quad_destination.iter()) {
        match std::fs::symlink_metadata(destination) {
            Ok(_) => {
                return Err(CoreError::Conflict(format!(
                    "CLI output already exists and was not overwritten: {}",
                    destination.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(CoreError::Io(error)),
        }
    }
    Ok(())
}

fn write_cli_output_with<F>(
    path: &std::path::Path,
    bytes: &[u8],
    write: F,
) -> Result<PathBuf, CoreError>
where
    F: FnOnce(&mut std::fs::File, &std::path::Path, &[u8]) -> std::io::Result<()>,
{
    let destination = canonical_cli_output_path(path)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                CoreError::Conflict(format!(
                    "CLI output already exists and was not overwritten: {}",
                    destination.display()
                ))
            } else {
                CoreError::Io(error)
            }
        })?;
    let write_result = write(&mut file, &destination, bytes).and_then(|()| file.sync_all());
    if let Err(error) = write_result {
        drop(file);
        let message = format!(
            "CLI output may be incomplete at {}: {error}",
            destination.display()
        );
        return Err(CoreError::Io(std::io::Error::new(error.kind(), message)));
    }
    Ok(destination)
}

/// Create a CLI output without following or replacing an existing file or symlink.
fn write_cli_output(path: &std::path::Path, bytes: &[u8]) -> Result<PathBuf, CoreError> {
    write_cli_output_with(path, bytes, |file, _, bytes| {
        std::io::Write::write_all(file, bytes)
    })
}

fn canonical_mcp_export_path(
    data_dir: &std::path::Path,
    output_path: &std::path::Path,
) -> Result<PathBuf, CoreError> {
    if !output_path.is_absolute() {
        return Err(CoreError::InvalidInput(
            "Export path must be absolute on the local MCP host.".into(),
        ));
    }
    let configured_export_root = std::env::var_os("MESHHARBOR_EXPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("exports"));
    std::fs::create_dir_all(&configured_export_root)?;
    let canonical_export_root = std::fs::canonicalize(&configured_export_root)?;
    let destination = canonical_cli_output_path(output_path)?;
    if !destination
        .parent()
        .is_some_and(|parent| parent.starts_with(&canonical_export_root))
    {
        return Err(CoreError::InvalidInput(format!(
            "MCP exports must stay inside the configured export directory: {}",
            canonical_export_root.display()
        )));
    }
    Ok(destination)
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

fn parse_overdraw_threshold(value: &str) -> Result<f32, String> {
    let threshold = value
        .parse::<f32>()
        .map_err(|error| format!("invalid overdraw threshold: {error}"))?;
    if !threshold.is_finite() || !(1.0..=1.5).contains(&threshold) {
        return Err("must be finite and between 1.0 and 1.5.".into());
    }
    Ok(threshold)
}

fn parse_cli_json<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|error| error.to_string())
}

fn validate_process_mesh_cli_options(
    operation: &str,
    target_quads: Option<u32>,
    has_quad_output: bool,
    resolution: Option<u32>,
    voxel_resolution: Option<u32>,
    padding: Option<u32>,
    overdraw_threshold: Option<f32>,
) -> Result<(), &'static str> {
    if operation != "mesh.retopology" && (target_quads.is_some() || has_quad_output) {
        return Err("--target-quads and --quad-output require mesh.retopology.");
    }
    if operation != "uv.unwrap" && (resolution.is_some() || padding.is_some()) {
        return Err("--resolution and --padding require uv.unwrap.");
    }
    if operation != "mesh.voxel_remesh" && voxel_resolution.is_some() {
        return Err("--voxel-resolution requires mesh.voxel_remesh.");
    }
    if operation != "render.optimize" && overdraw_threshold.is_some() {
        return Err("--overdraw-threshold requires render.optimize.");
    }
    Ok(())
}

fn process_mesh_cli_parameters(
    operation: &str,
    target_quads: Option<u32>,
    resolution: Option<u32>,
    voxel_resolution: Option<u32>,
    padding: Option<u32>,
    overdraw_threshold: Option<f32>,
) -> Value {
    let mut parameters = json!({});
    match operation {
        "mesh.retopology" => {
            if let Some(target_quads) = target_quads {
                parameters["target_quads"] = json!(target_quads);
            }
        }
        "uv.unwrap" => {
            let defaults = meshharbor_core::AtlasBakeOptions::default();
            parameters["resolution"] = json!(resolution.unwrap_or(defaults.resolution));
            parameters["padding"] = json!(padding.unwrap_or(defaults.padding));
        }
        "mesh.voxel_remesh" => {
            if let Some(voxel_resolution) = voxel_resolution {
                parameters["resolution"] = json!(voxel_resolution);
            }
        }
        "render.optimize" => {
            if let Some(threshold) = overdraw_threshold {
                parameters["overdraw_threshold"] = json!(threshold);
            }
        }
        _ => {}
    }
    parameters
}

fn default_max_error() -> f32 {
    0.02
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

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn decode_base64(encoded: &str) -> Result<Vec<u8>, String> {
    decode_base64_with_limit(encoded, MAX_ASSET_BYTES)
}

fn decode_base64_with_limit(encoded: &str, max_decoded_bytes: usize) -> Result<Vec<u8>, String> {
    let raw = encoded.as_bytes();
    if !raw.len().is_multiple_of(4) {
        return Err("Base64 input length must be divisible by four.".into());
    }
    let max_encoded_bytes = max_decoded_bytes
        .div_ceil(3)
        .checked_mul(4)
        .ok_or_else(|| "Base64 size limit is too large.".to_string())?;
    if raw.len() > max_encoded_bytes {
        return Err(format!(
            "Base64 asset exceeds the {max_decoded_bytes}-byte limit."
        ));
    }

    let mut output = Vec::with_capacity((raw.len() / 4 * 3).min(max_decoded_bytes));
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
                values[index] = base64_value(*byte)
                    .ok_or_else(|| "Base64 contains a non-alphabet character.".to_string())?;
            }
        }
        if padding > 2 {
            return Err("Base64 padding is invalid.".into());
        }
        let bytes_to_append = 3 - padding;
        if output
            .len()
            .checked_add(bytes_to_append)
            .is_none_or(|length| length > max_decoded_bytes)
        {
            return Err(format!(
                "Base64 asset exceeds the {max_decoded_bytes}-byte limit."
            ));
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

    fn minimal_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x01, 0x01, 0x05, 0x00, 0xfa, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00,
            0x01, 0x64, 0x78, 0x95, 0x38, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
            0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn base64_asset_decoding_honors_size_limit() {
        assert_eq!(decode_base64("AQID").unwrap(), [1, 2, 3]);
        assert_eq!(decode_base64("AQI=").unwrap(), [1, 2]);
        assert!(decode_base64_with_limit("AQID", 2).is_err());
        assert!(decode_base64("A===").is_err());
    }

    #[tokio::test]
    async fn cli_asset_reader_never_reads_past_configured_limit() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-bounded-read-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("asset.bin");
        std::fs::write(&path, b"12345").unwrap();

        assert_eq!(read_bounded_asset(&path, 5).await.unwrap(), b"12345");
        assert!(matches!(
            read_bounded_asset(&path, 4).await.unwrap_err(),
            CoreError::InvalidInput(_)
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn api_asset_page_filters_and_returns_a_cursor() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-asset-page-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let bytes = minimal_png();
        let source = core.import_asset("Oak%Panel Source.png", &bytes).unwrap();
        let exact = core
            .store()
            .import_derived_asset("Oak%Panel Derived.png", &bytes, &source.id, json!({}))
            .unwrap();
        core.store()
            .import_derived_asset("OakZZPanel Derived.png", &bytes, &source.id, json!({}))
            .unwrap();
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: Arc::new(JobScheduler::default()),
            },
            "token".into(),
        );

        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/assets?limit=1&q=oak%25panel&kind=image&derived=true")
                    .header(header::HOST, "localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let page: meshharbor_core::AssetPage =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|asset| asset.id.as_str())
                .collect::<Vec<_>>(),
            vec![exact.id.as_str()]
        );
        assert_eq!(page.next_cursor, None);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/assets?limit=1")
                    .header(header::HOST, "localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let page: meshharbor_core::AssetPage =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.next_cursor.is_some());

        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn model_inventory_lists_current_versions_and_restore_cannot_cross_models() {
        let root = std::env::temp_dir().join(format!(
            "meshharbor-model-versions-{}",
            uuid::Uuid::new_v4()
        ));
        let core = OperationCore::open(&root).unwrap();
        let bytes = minimal_triangle_glb();
        let original = core.import_asset("Original.glb", &bytes).unwrap();
        let other = core.import_asset("Other.glb", &bytes).unwrap();
        let edited = core
            .store()
            .import_derived_asset(
                "New filename.glb",
                &bytes,
                &original.id,
                json!({"operation":"mesh.cleanup"}),
            )
            .unwrap();
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: Arc::new(JobScheduler::default()),
            },
            "token".into(),
        );
        let request = |method: Method, uri: String, body: Body| {
            let app = app.clone();
            async move {
                let response = app
                    .oneshot(
                        axum::http::Request::builder()
                            .method(method)
                            .uri(uri)
                            .header(header::HOST, "localhost:7432")
                            .header(header::AUTHORIZATION, "Bearer token")
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(body)
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let status = response.status();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                (status, serde_json::from_slice::<Value>(&bytes).unwrap())
            }
        };
        let (status, page) = request(
            Method::GET,
            "/api/v1/models?q=original".into(),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["id"], edited.id);
        assert_eq!(page["items"][0]["model"]["name"], original.name);
        let current_path = format!("/api/v1/models/{}/current", original.id);
        let (status, _) = request(
            Method::PUT,
            current_path.clone(),
            Body::from(json!({"asset_id":other.id}).to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, restored) = request(
            Method::PUT,
            current_path,
            Body::from(json!({"asset_id":original.id}).to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(restored["model"]["current_asset_id"], original.id);
        let branch = core
            .store()
            .import_derived_asset(
                "Parted branch.glb",
                &bytes,
                &edited.id,
                json!({"operation":"mesh.separate_parts"}),
            )
            .unwrap();
        let (status, history) = request(
            Method::GET,
            format!("/api/v1/models/{}/versions", original.id),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(history["current_asset_id"], original.id);
        assert_eq!(history["items"][0]["asset_id"], branch.id);
        assert_eq!(history["items"][0]["parent_number"], 2);
        assert_eq!(history["items"].as_array().unwrap().len(), 3);
        assert_eq!(
            core.store()
                .read_artifact(&edited.artifacts[0].id)
                .unwrap()
                .bytes,
            bytes
        );
        drop(app);
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn retired_connected_part_jobs_are_read_only() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-retired-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let bytes = minimal_triangle_glb();
        let source = core.import_asset("Original.glb", &bytes).unwrap();
        let operation = "mesh.separate_parts";
        let historical = core
            .store()
            .import_derived_asset(
                "Historical parts.glb",
                &bytes,
                &source.id,
                json!({"operation": operation}),
            )
            .unwrap();
        let completed = core
            .store()
            .create_running_job(operation, Some(&source.id), &json!({}))
            .unwrap();
        core.store()
            .finish_job(&completed.id, Ok(json!({ "asset_id": historical.id })))
            .unwrap();
        let failed = core
            .store()
            .create_running_job(operation, Some(&source.id), &json!({}))
            .unwrap();
        core.store()
            .finish_job(
                &failed.id,
                Err(CoreError::OperationUnavailable("Old failure.".into())),
            )
            .unwrap();
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: Arc::new(JobScheduler::default()),
            },
            "token".into(),
        );
        let request = |method: Method, uri: String, body: Body| {
            app.clone().oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::HOST, "localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .unwrap(),
            )
        };
        let completed_response = request(
            Method::GET,
            format!("/api/v1/jobs/{}", completed.id),
            Body::empty(),
        )
        .await
        .unwrap();
        assert_eq!(completed_response.status(), StatusCode::OK);
        let completed_record: meshharbor_core::Job = serde_json::from_slice(
            &completed_response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes(),
        )
        .unwrap();
        assert_eq!(completed_record.result.unwrap()["asset_id"], historical.id);
        let artifact = request(
            Method::GET,
            format!("/api/v1/artifacts/{}", historical.artifacts[0].id),
            Body::empty(),
        )
        .await
        .unwrap();
        assert_eq!(artifact.status(), StatusCode::OK);
        assert_eq!(
            artifact
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .as_ref(),
            bytes
        );

        let new_job = request(
            Method::POST,
            "/api/v1/jobs".into(),
            Body::from(
                json!({
                    "operation": operation,
                    "input": { "asset_id": source.id },
                    "parameters": {}
                })
                .to_string(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(new_job.status(), StatusCode::METHOD_NOT_ALLOWED);
        let retry = request(
            Method::POST,
            format!("/api/v1/jobs/{}/retry", failed.id),
            Body::from("{}"),
        )
        .await
        .unwrap();
        assert_eq!(retry.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(core.store().list_jobs(10).unwrap().len(), 2);
        drop(app);
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn api_rejects_malformed_png_without_persisting_an_asset() {
        let root = std::env::temp_dir().join(format!(
            "meshharbor-invalid-upload-{}",
            uuid::Uuid::new_v4()
        ));
        let core = OperationCore::open(&root).unwrap();
        let app = api_router(
            AppState {
                core,
                scheduler: Arc::new(JobScheduler::default()),
            },
            "token".into(),
        );
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/assets?filename=truncated.png")
                    .header(header::HOST, "localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .body(Body::from(b"\x89PNG\r\n\x1a\n".to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let reopened = OperationCore::open(&root).unwrap();
        assert!(reopened.store().list_assets(10).unwrap().is_empty());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn generation_queue_api_swaps_only_live_waiting_generation_jobs() {
        let root = std::env::temp_dir().join(format!("meshharbor-queue-{}", uuid::Uuid::new_v4()));
        let scheduler = Arc::new(JobScheduler::default());
        let core = OperationCore::open(&root).unwrap();
        let first = core
            .store()
            .create_job("image.generate", None, &json!({}))
            .unwrap();
        let mesh = core
            .store()
            .create_job("mesh.cleanup", None, &json!({}))
            .unwrap();
        let second = core
            .store()
            .create_job("image.generate", None, &json!({}))
            .unwrap();
        let interrupted = core
            .store()
            .create_job("image.generate", None, &json!({}))
            .unwrap();
        for job in [&first, &mesh, &second] {
            scheduler.enqueue(&job.id, &job.operation);
        }
        let app = api_router(
            AppState {
                core,
                scheduler: scheduler.clone(),
            },
            "queue-token".into(),
        );
        let request = |method, path: &str, body: Body| {
            axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header(header::HOST, "localhost:7432")
                .header(header::AUTHORIZATION, "Bearer queue-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .unwrap()
        };
        let endpoint = "/api/v1/generation/queue";
        let response = app
            .clone()
            .oneshot(request(Method::GET, endpoint, Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!([first.id, second.id])
        );
        let swap = |other: &str| {
            request(
                Method::POST,
                endpoint,
                Body::from(json!({"job_id":first.id,"swap_with":other}).to_string()),
            )
        };
        for unavailable in [&interrupted.id, &mesh.id] {
            assert_eq!(
                app.clone()
                    .oneshot(swap(unavailable))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::CONFLICT
            );
        }
        let response = app.clone().oneshot(swap(&second.id)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!([second.id, first.id])
        );
        let cancel_path = format!("/api/v1/jobs/{}/cancel?queued_only=true", second.id);
        assert_eq!(
            app.oneshot(request(Method::POST, &cancel_path, Body::empty()))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(scheduler.generation_queue(), [first.id]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn connected_cli_exposes_features_not_generic_job_submission() {
        for args in [
            vec!["meshharbor", "engine", "runtime", "quadriflow"],
            vec![
                "meshharbor",
                "engine",
                "mesh",
                "simplify",
                "asset_demo",
                "--target-triangles",
                "40",
            ],
            vec!["meshharbor", "engine", "generation", "image", "asset_demo"],
            vec!["meshharbor", "engine", "generation", "text", "a cup"],
            vec!["meshharbor", "engine", "models", "list"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        for args in [
            vec!["meshharbor", "engine", "jobs", "submit"],
            vec!["meshharbor", "runtime", "setup"],
            vec!["meshharbor", "import", "./test.glb"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
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
    fn process_mesh_cli_parses_uv_and_render_settings_with_bounds() {
        let uv = Cli::try_parse_from([
            "meshharbor",
            "local",
            "process-mesh",
            "asset_example",
            "--operation",
            "uv.unwrap",
            "--resolution",
            "2048",
            "--padding",
            "12",
        ])
        .unwrap();
        assert!(matches!(
            uv.command,
            Command::Local {
                command: LocalCommand::ProcessMesh {
                    resolution: Some(2048),
                    padding: Some(12),
                    ..
                }
            }
        ));

        let render = Cli::try_parse_from([
            "meshharbor",
            "local",
            "process-mesh",
            "asset_example",
            "--operation",
            "render.optimize",
            "--overdraw-threshold",
            "1.2",
        ])
        .unwrap();
        assert!(matches!(
            render.command,
            Command::Local { command: LocalCommand::ProcessMesh {
                overdraw_threshold: Some(value),
                ..
            } } if (value - 1.2).abs() < 0.0001
        ));

        assert!(
            Cli::try_parse_from([
                "meshharbor",
                "local",
                "process-mesh",
                "asset_example",
                "--operation",
                "uv.unwrap",
                "--resolution",
                "63",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "meshharbor",
                "local",
                "process-mesh",
                "asset_example",
                "--operation",
                "uv.unwrap",
                "--padding",
                "65",
            ])
            .is_err()
        );

        for (resolution, padding) in [("64", "0"), ("4096", "64")] {
            let cli = Cli::try_parse_from([
                "meshharbor",
                "local",
                "process-mesh",
                "asset_example",
                "--operation",
                "uv.unwrap",
                "--resolution",
                resolution,
                "--padding",
                padding,
            ])
            .unwrap();
            let expected_resolution = resolution.parse::<u32>().unwrap();
            let expected_padding = padding.parse::<u32>().unwrap();
            assert!(matches!(
                cli.command,
                Command::Local { command: LocalCommand::ProcessMesh {
                    resolution: Some(parsed_resolution),
                    padding: Some(parsed_padding),
                    ..
                } } if parsed_resolution == expected_resolution
                    && parsed_padding == expected_padding
            ));
        }
        assert!(
            Cli::try_parse_from([
                "meshharbor",
                "local",
                "process-mesh",
                "asset_example",
                "--operation",
                "render.optimize",
                "--overdraw-threshold",
                "1.6",
            ])
            .is_err()
        );

        for (value, expected) in [("1.0", 1.0), ("1.5", 1.5)] {
            let cli = Cli::try_parse_from([
                "meshharbor",
                "local",
                "process-mesh",
                "asset_example",
                "--operation",
                "render.optimize",
                "--overdraw-threshold",
                value,
            ])
            .unwrap();
            assert!(matches!(
                cli.command,
                Command::Local { command: LocalCommand::ProcessMesh {
                    overdraw_threshold: Some(parsed),
                    ..
                } } if parsed == expected
            ));
        }
        for value in ["NaN", "inf", "0.99", "1.51"] {
            assert!(
                Cli::try_parse_from([
                    "meshharbor",
                    "local",
                    "process-mesh",
                    "asset_example",
                    "--operation",
                    "render.optimize",
                    "--overdraw-threshold",
                    value,
                ])
                .is_err(),
                "accepted invalid overdraw threshold {value}"
            );
        }
    }

    #[test]
    fn process_mesh_cli_settings_are_operation_scoped_and_forwarded() {
        assert!(
            validate_process_mesh_cli_options(
                "mesh.cleanup",
                Some(100),
                false,
                None,
                None,
                None,
                None
            )
            .is_err()
        );
        assert!(
            validate_process_mesh_cli_options(
                "mesh.cleanup",
                None,
                false,
                Some(2048),
                None,
                None,
                None
            )
            .is_err()
        );
        assert!(
            validate_process_mesh_cli_options(
                "mesh.cleanup",
                None,
                false,
                None,
                None,
                None,
                Some(1.2)
            )
            .is_err()
        );
        assert!(
            validate_process_mesh_cli_options(
                "uv.unwrap",
                None,
                false,
                Some(2048),
                None,
                Some(12),
                None
            )
            .is_ok()
        );
        assert!(
            validate_process_mesh_cli_options("uv.unwrap", None, false, None, Some(80), None, None)
                .is_err()
        );
        assert!(
            validate_process_mesh_cli_options(
                "mesh.voxel_remesh",
                None,
                false,
                None,
                Some(80),
                None,
                None
            )
            .is_ok()
        );
        assert!(
            validate_process_mesh_cli_options(
                "render.optimize",
                None,
                false,
                None,
                None,
                None,
                Some(1.2)
            )
            .is_ok()
        );

        let automatic =
            process_mesh_cli_parameters("mesh.retopology", None, None, None, None, None);
        assert_eq!(automatic, json!({}));
        let targeted =
            process_mesh_cli_parameters("mesh.retopology", Some(12), None, None, None, None);
        assert_eq!(targeted, json!({"target_quads": 12}));
        let uv_defaults = process_mesh_cli_parameters("uv.unwrap", None, None, None, None, None);
        assert_eq!(uv_defaults["resolution"], 1024);
        assert_eq!(uv_defaults["padding"], 8);
        let uv_custom =
            process_mesh_cli_parameters("uv.unwrap", None, Some(2048), None, Some(12), None);
        assert_eq!(uv_custom["resolution"], 2048);
        assert_eq!(uv_custom["padding"], 12);
        let voxel =
            process_mesh_cli_parameters("mesh.voxel_remesh", None, None, Some(80), None, None);
        assert_eq!(voxel, json!({"resolution": 80}));
        let render =
            process_mesh_cli_parameters("render.optimize", None, None, None, None, Some(1.2));
        let forwarded_threshold = render["overdraw_threshold"].as_f64().unwrap();
        assert!((forwarded_threshold - 1.2).abs() < 0.000001);
    }

    #[test]
    fn process_mesh_cli_parses_bounded_voxel_resolution() {
        for (value, valid) in [("15", false), ("16", true), ("160", true), ("161", false)] {
            let parsed = Cli::try_parse_from([
                "meshharbor",
                "local",
                "process-mesh",
                "asset_example",
                "--operation",
                "mesh.voxel_remesh",
                "--voxel-resolution",
                value,
            ]);
            assert_eq!(parsed.is_ok(), valid, "{value}");
        }
    }

    #[test]
    fn jobs_cli_parses_read_only_history_commands() {
        let list = Cli::try_parse_from(["meshharbor", "engine", "jobs", "list", "--limit", "200"])
            .unwrap();
        assert!(matches!(
            list.command,
            Command::Engine {
                command: engine_client::Command::Jobs {
                    action: engine_client::Jobs::List {
                        limit: Some(200),
                        ..
                    }
                },
                ..
            }
        ));
        assert!(
            Cli::try_parse_from(["meshharbor", "engine", "jobs", "show", "job_example"]).is_ok()
        );
    }

    #[test]
    fn job_history_queries_are_read_only_and_persist_across_reopen() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-job-history-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let queued = core
            .store()
            .create_job("mesh.cleanup", None, &json!({}))
            .unwrap();
        let running = core
            .store()
            .create_job("mesh.simplify", None, &json!({}))
            .unwrap();
        core.store()
            .update_job(
                &running.id,
                meshharbor_core::JobState::Running,
                25,
                None,
                None,
            )
            .unwrap();

        let before = read_job_history(&core, 200).unwrap();
        assert_eq!(read_job_history(&core, 0).unwrap().len(), 1);
        assert_eq!(
            read_job_record(&core, "job_missing")
                .unwrap_err()
                .to_string(),
            "not found: Job 'job_missing' was not found."
        );
        for job in &before {
            let current = read_job_record(&core, &job.id).unwrap();
            assert_eq!(current.state, job.state);
            assert_eq!(current.progress, job.progress);
            assert_eq!(current.updated_at, job.updated_at);
        }

        drop(core);
        let reopened = OperationCore::open(&root).unwrap();
        let persisted = read_job_history(&reopened, 200).unwrap();
        assert!(persisted.iter().any(|job| job.id == queued.id));
        let persisted_running = read_job_record(&reopened, &running.id).unwrap();
        assert_eq!(persisted_running.state, meshharbor_core::JobState::Running);
        assert_eq!(persisted_running.progress, 25);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_job_recovery_requires_an_explicit_stopped_process_acknowledgement() {
        assert!(Cli::try_parse_from(["meshharbor", "recover-jobs"]).is_err());
        assert!(
            Cli::try_parse_from([
                "meshharbor",
                "recover-jobs",
                "--confirm-no-running-meshharbor",
            ])
            .is_ok()
        );
    }

    #[tokio::test]
    async fn startup_records_tool_detection_without_queuing_a_download() {
        let root = std::env::temp_dir().join(format!("meshharbor-detect-{}", uuid::Uuid::new_v4()));
        let service = open_job_service(&root).await.unwrap();
        assert!(root.join("model-environment/tools-v2.json").is_file());
        assert!(!root.join("model-environment/huggingface").exists());
        assert!(service.state.core.store().list_jobs(20).unwrap().is_empty());
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn startup_recovers_dead_owners_without_disturbing_a_live_service() {
        use data_dir_lock::DataDirLock;
        use meshharbor_core::JobState;

        let root =
            std::env::temp_dir().join(format!("meshharbor-job-owner-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let legacy = core
            .store()
            .create_running_job("mesh.cleanup", None, &json!({}))
            .unwrap();
        let live = open_job_service(&root).await.unwrap();
        assert_eq!(
            read_job_record(&core, &legacy.id).unwrap().state,
            JobState::Failed
        );
        let (live_job, live_permit) = live
            .state
            .scheduler
            .submit(live.state.core.store(), "mesh.cleanup", None, &json!({}))
            .unwrap();
        let dead_owner = uuid::Uuid::new_v4();
        let dead_lock = DataDirLock::try_acquire_for_job_owner(&root, dead_owner).unwrap();
        let interrupted = core
            .store()
            .create_owned_job(
                RuntimeJobKind::InstallCatalogModel("pixal3d-mlx").operation(),
                None,
                &json!({}),
                true,
                Some(&dead_owner.to_string()),
            )
            .unwrap();
        core.store()
            .request_job_cancellation(&interrupted.id, false)
            .unwrap();
        drop(dead_lock);

        let restarted = open_job_service(&root).await.unwrap();
        assert_eq!(
            read_job_record(&restarted.state.core, &live_job.id)
                .unwrap()
                .state,
            JobState::Running
        );
        let recovered = read_job_record(&restarted.state.core, &interrupted.id).unwrap();
        assert_eq!(recovered.state, JobState::Failed);
        assert_eq!(recovered.error.unwrap()["previous_state"], "cancelling");

        drop(live_permit);
        drop((core, restarted, live));
        std::fs::remove_dir_all(root).unwrap();
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
    async fn huggingface_token_connection_requires_same_origin_studio_and_redacts_bad_input() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-hf-access-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: Arc::new(JobScheduler::default()),
            },
            "private-test-token".into(),
        );
        for scope in ["mlx", "cuda"] {
            for (origin, dashboard, bearer, status) in [
                (
                    Some("http://127.0.0.1:5173"),
                    true,
                    false,
                    StatusCode::FORBIDDEN,
                ),
                (None, true, false, StatusCode::FORBIDDEN),
                (None, false, true, StatusCode::UNAUTHORIZED),
                (
                    Some("http://127.0.0.1:7432"),
                    false,
                    true,
                    StatusCode::UNAUTHORIZED,
                ),
                (
                    Some("http://127.0.0.1:7432"),
                    true,
                    false,
                    StatusCode::BAD_REQUEST,
                ),
            ] {
                let mut request = axum::http::Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/connections/huggingface/{scope}"))
                    .header(header::HOST, "127.0.0.1:7432")
                    .header(header::CONTENT_TYPE, "application/json");
                if let Some(origin) = origin {
                    request = request.header(header::ORIGIN, origin);
                }
                if dashboard {
                    request = request.header("x-meshharbor-dashboard", "local-ui-v1");
                }
                if bearer {
                    request = request.header(header::AUTHORIZATION, "Bearer private-test-token");
                }
                let response = app
                    .clone()
                    .oneshot(
                        request
                            .body(Body::from(r#"{"hf_secret_fixture":true}"#))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), status);
                let body = response.into_body().collect().await.unwrap().to_bytes();
                assert!(!String::from_utf8_lossy(&body).contains("hf_secret_fixture"));
            }
        }
        assert!(core.store().list_jobs(20).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn catalog_installs_require_authorization_and_confirmation() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-catalog-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: Arc::new(JobScheduler::default()),
            },
            "private-test-token".into(),
        );
        for (backend, confirmed, dashboard, bearer, expected) in [
            ("trellis2-mlx", true, false, false, StatusCode::UNAUTHORIZED),
            ("trellis2-mlx", false, true, false, StatusCode::BAD_REQUEST),
            ("trellis2-mlx", true, false, true, StatusCode::BAD_REQUEST),
            ("geosam2-mlx", false, false, true, StatusCode::BAD_REQUEST),
            ("geosam2-cuda", false, true, false, StatusCode::BAD_REQUEST),
            ("unknown-mlx", true, false, true, StatusCode::BAD_REQUEST),
        ] {
            let mut request = axum::http::Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/runtime/{backend}/install"))
                .header(header::HOST, "127.0.0.1:7432")
                .header(header::ORIGIN, "http://127.0.0.1:7432")
                .header(header::CONTENT_TYPE, "application/json");
            if dashboard {
                request = request.header("x-meshharbor-dashboard", "local-ui-v1");
            }
            if bearer {
                request = request.header(header::AUTHORIZATION, "Bearer private-test-token");
            }
            let response = app
                .clone()
                .oneshot(
                    request
                        .body(Body::from(
                            json!({"confirmed": confirmed, "confirm_gated_access": false})
                                .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        assert!(core.store().list_jobs(20).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn removed_submit_routes_and_unapproved_download_retry_cannot_queue_jobs() {
        let root = tempfile::tempdir().unwrap();
        let core = OperationCore::open(root.path()).unwrap();
        let saved = core
            .store()
            .create_running_job(
                RuntimeJobKind::CudaInstallMultiview.operation(),
                None,
                &json!({}),
            )
            .unwrap();
        core.store()
            .finish_job(&saved.id, Err(CoreError::Cancelled))
            .unwrap();
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: Arc::default(),
            },
            "token".into(),
        );
        for (path, payload, expected) in [
            (
                "/api/v1/jobs".to_owned(),
                json!({"operation":"mesh.cleanup"}),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                "/api/v1/runtime/pixal3d-mlx/setup".to_owned(),
                json!({}),
                StatusCode::NOT_FOUND,
            ),
            (
                "/api/v1/assets/example/cleanup".to_owned(),
                json!({"target_triangles":32}),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/api/v1/runtime/pixal3d-mlx/install".to_owned(),
                json!({"confirmed":true,"package":"multiview"}),
                StatusCode::BAD_REQUEST,
            ),
            (
                format!("/api/v1/jobs/{}/retry", saved.id),
                json!({}),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method(Method::POST)
                        .uri(&path)
                        .header(header::HOST, "localhost:7432")
                        .header(header::AUTHORIZATION, "Bearer token")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(payload.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{path}");
        }
        assert_eq!(core.store().list_jobs(10).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn environment_setup_requires_authorization_and_explicit_confirmation() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-environment-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: Arc::new(JobScheduler::default()),
            },
            "private-test-token".into(),
        );
        for (confirmed, dashboard, bearer, origin, expected) in [
            (
                true,
                false,
                false,
                Some("http://127.0.0.1:7432"),
                StatusCode::UNAUTHORIZED,
            ),
            (
                false,
                true,
                false,
                Some("http://127.0.0.1:7432"),
                StatusCode::BAD_REQUEST,
            ),
            (false, false, true, None, StatusCode::BAD_REQUEST),
            (
                true,
                false,
                true,
                Some("http://127.0.0.1:5173"),
                StatusCode::FORBIDDEN,
            ),
        ] {
            let mut request = axum::http::Request::builder()
                .method(Method::POST)
                .uri("/api/v1/runtime/environment/setup")
                .header(header::HOST, "127.0.0.1:7432")
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            if bearer {
                request = request.header(header::AUTHORIZATION, "Bearer private-test-token");
            }
            if dashboard {
                request = request.header("x-meshharbor-dashboard", "local-ui-v1");
            }
            let response = app
                .clone()
                .oneshot(
                    request
                        .body(Body::from(json!({"confirmed":confirmed}).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        assert!(core.store().list_jobs(20).unwrap().is_empty());
        assert!(!core.runtime_doctor().environment.ready);
        drop(app);
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repart_catalog_installs_use_one_job_without_enabling_studio_repart() {
        let root = std::env::temp_dir().join(format!("meshharbor-repart-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let doctor = core.runtime_doctor();
        let repart = doctor
            .model_catalog
            .iter()
            .find(|model| model["id"] == "geosam2")
            .unwrap();
        assert_eq!(repart["category"], "repart");
        assert_eq!(
            repart["backend_ids"],
            json!(["geosam2-mlx", "geosam2-cuda"])
        );
        for backend in doctor
            .model_backends
            .iter()
            .filter(|backend| backend["id"].as_str().unwrap().starts_with("geosam2-"))
        {
            assert_eq!(backend["ready"], false);
            assert!(backend.get("generation").is_none());
            let id = backend["id"].as_str().unwrap();
            let job = core.catalog_install_job(id, false);
            if backend["installable"] == true {
                let job = job.unwrap();
                assert_eq!(job.operation(), "runtime.models.install");
                assert_eq!(job.backend_id(), id);
            } else {
                assert!(matches!(job, Err(CoreError::OperationUnavailable(_))));
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn settings_removal_is_confirmed_scoped_and_runtime_locked() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-settings-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let mlx = root.join("runtimes/pixal3d-mlx");
        let hf = mlx.join("home/.cache/huggingface");
        let partial = hf.join("hub/models--TencentARC--Pixal3D/blobs/partial.incomplete");
        let dino = hf.join("hub/models--facebook--dinov3-vitl16-pretrain-lvd1689m/blobs/pinned");
        let xet = hf.join("xet/cache");
        let naf = mlx.join("weights/naf_release.safetensors");
        let token = hf.join("token");
        let named_tokens = hf.join("stored_tokens");
        let environment = mlx.join(".venv/bin/python");
        let other_repo = hf.join("hub/models--keep--unrelated/blob");
        let other_profile = root.join("runtimes/pixal3d-cuda/home/.cache/huggingface/token");
        let artifact = root.join("artifacts/keep");
        for path in [
            &partial,
            &dino,
            &xet,
            &naf,
            &token,
            &named_tokens,
            &environment,
            &other_repo,
            &other_profile,
            &artifact,
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"fixture data").unwrap();
        }
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: Arc::new(JobScheduler::default()),
            },
            "private-test-token".into(),
        );
        let request = |scope: &str, origin: &str, confirmed: bool| {
            let path = if scope == "huggingface" {
                "/api/v1/connections/huggingface/mlx"
            } else {
                "/api/v1/runtime/pixal3d-mlx/model-files"
            };
            axum::http::Request::builder()
                .method(Method::DELETE)
                .uri(path)
                .header(header::HOST, "127.0.0.1:7432")
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-meshharbor-dashboard", "local-ui-v1")
                .body(Body::from(json!({"confirmed": confirmed}).to_string()))
                .unwrap()
        };
        let same_origin = "http://127.0.0.1:7432";
        let generation = acquire_generation_runtime_lock(&root).unwrap();
        for (origin, confirmed, expected) in [
            ("http://127.0.0.1:5173", true, StatusCode::FORBIDDEN),
            (same_origin, false, StatusCode::BAD_REQUEST),
            (same_origin, true, StatusCode::CONFLICT),
        ] {
            let response = app
                .clone()
                .oneshot(request("model-files", origin, confirmed))
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            assert!(partial.is_file());
        }
        drop(generation);

        let removed = app
            .clone()
            .oneshot(request("model-files", same_origin, true))
            .await
            .unwrap();
        assert_eq!(removed.status(), StatusCode::OK);
        let info: RuntimeDoctor =
            serde_json::from_slice(&removed.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let mlx_status = info
            .model_backends
            .iter()
            .find(|backend| backend["id"] == "pixal3d-mlx")
            .unwrap();
        assert_eq!(mlx_status["local_files"]["remove_models_available"], false);
        assert_eq!(mlx_status["local_files"]["token_saved"], true);
        assert!(
            ![&partial, &dino, &xet, &naf]
                .into_iter()
                .any(|path| path.exists())
        );
        assert!(
            [&token, &named_tokens, &environment, &other_repo, &artifact]
                .into_iter()
                .all(|path| path.is_file())
        );

        let disconnected = app
            .oneshot(request("huggingface", same_origin, true))
            .await
            .unwrap();
        assert_eq!(disconnected.status(), StatusCode::OK);
        assert!(!token.exists());
        assert!(!named_tokens.exists());
        assert!(other_profile.is_file());
        assert!(other_repo.is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn preflight_works_without_a_backend_and_does_not_create_jobs() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-preflight-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let asset = core
            .import_asset("open-triangle.glb", &minimal_triangle_glb())
            .unwrap();
        let scheduler = Arc::new(JobScheduler::default());
        assert!(!core.quadriflow_backend_status().available);
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: scheduler.clone(),
            },
            "private-test-token".into(),
        );
        let endpoint = format!("/api/v1/assets/{}/retopology-preflight", asset.id);
        let request = |uri: &str, body: &'static str, authorized: bool| {
            let mut builder = axum::http::Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::HOST, "localhost:7432")
                .header(header::CONTENT_TYPE, "application/json");
            if authorized {
                builder = builder
                    .header(header::ORIGIN, "http://localhost:7432")
                    .header("x-meshharbor-dashboard", "local-ui-v1");
            }
            builder.body(Body::from(body)).unwrap()
        };
        let denied = app
            .clone()
            .oneshot(request(&endpoint, "{}", false))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let invalid = app
            .clone()
            .oneshot(request(&endpoint, r#"{"target_quads":3}"#, true))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let wrong_geometry = app
            .clone()
            .oneshot(request(&endpoint, r#"{"target_quads":12}"#, true))
            .await
            .unwrap();
        assert_eq!(wrong_geometry.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = wrong_geometry
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let error: Value = serde_json::from_slice(&body).unwrap();
        let expected = core
            .preflight_retopology_asset(&asset.id, retopology_options(Some(12)))
            .unwrap_err();
        assert_eq!(error["error"], expected.to_string());
        assert!(!expected.to_string().contains("sidecar"));

        let permit = scheduler.try_acquire_owned().unwrap();
        let busy = app.oneshot(request(&endpoint, "{}", true)).await.unwrap();
        assert_eq!(busy.status(), StatusCode::CONFLICT);
        drop(permit);
        assert!(core.store().list_jobs(10).unwrap().is_empty());
        let existing = core.store().list_assets(10).unwrap();
        assert_eq!(existing.len(), 1);
        assert_eq!(existing[0].sha256, asset.sha256);
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn rest_api_allows_health_and_same_origin_dashboard_but_rejects_unproven_requests() {
        let root = std::env::temp_dir().join(format!("meshharbor-api-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let app = api_router(
            AppState {
                core,
                scheduler: Arc::new(JobScheduler::default()),
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
                    .uri("/api/v1/runtime/quadriflow")
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
        assert_eq!(backend_json["available"], false);
        let unauthenticated_install = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/runtime/quadriflow/install")
                    .header(header::HOST, "localhost:7432")
                    .header(header::ORIGIN, "http://localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated_install.status(), StatusCode::UNAUTHORIZED);
        if backend_json["install_available"] == false {
            let backend_install = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/api/v1/runtime/quadriflow/install")
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
        }
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
                    .body(Body::from(minimal_png()))
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
                    .body(Body::from(minimal_png()))
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
                    .body(Body::from(minimal_png()))
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
                scheduler: Arc::new(JobScheduler::default()),
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
                scheduler: Arc::new(JobScheduler::default()),
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
        planar_patch_glb(false)
    }

    fn planar_patch_glb(subdivided: bool) -> Vec<u8> {
        let mut binary = Vec::new();
        let mut positions = vec![
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
        ];
        let indices: &[u16] = if subdivided {
            positions.push([0.5, 0.0, 0.5]);
            &[0, 2, 4, 2, 3, 4, 3, 1, 4, 1, 0, 4]
        } else {
            &[0, 2, 1, 1, 2, 3]
        };
        for position in &positions {
            for component in position {
                binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        let index_offset = binary.len();
        for index in indices {
            binary.extend_from_slice(&index.to_le_bytes());
        }
        let document = json!({
            "asset":{"version":"2.0"},
            "buffers":[{"byteLength":binary.len()}],
            "bufferViews":[
                {"buffer":0,"byteOffset":0,"byteLength":index_offset,"byteStride":12,"target":34962},
                {"buffer":0,"byteOffset":index_offset,"byteLength":indices.len() * 2,"target":34963}
            ],
            "accessors":[
                {"bufferView":0,"componentType":5126,"count":positions.len(),"type":"VEC3"},
                {"bufferView":1,"componentType":5123,"count":indices.len(),"type":"SCALAR"}
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
    async fn unreachable_simplification_limit_fails_the_job_without_creating_an_asset() {
        let root = std::env::temp_dir().join(format!("meshharbor-cap-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let source = core
            .import_asset("square.glb", &minimal_triangle_glb())
            .unwrap();
        let submitted = start_simplify_job(
            core.clone(),
            Arc::new(JobScheduler::default()),
            &source.id,
            1,
            1.0,
        )
        .await
        .unwrap();
        let mut job = submitted;
        for _ in 0..100 {
            job = core.store().get_job(&job.id).unwrap().unwrap();
            if job.state.is_terminal() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(job.state, meshharbor_core::JobState::Failed);
        assert!(job.result.is_none());
        assert!(
            job.error.unwrap()["message"]
                .as_str()
                .unwrap()
                .contains("1-triangle limit")
        );
        assert_eq!(core.store().list_assets(10).unwrap().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn api_job_returns_accepted_and_completes_against_the_shared_core() {
        let root = std::env::temp_dir().join(format!("meshharbor-job-{}", uuid::Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let source = core
            .import_asset("square.glb", &planar_patch_glb(true))
            .unwrap();
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: Arc::new(JobScheduler::default()),
            },
            "token".into(),
        );
        let body = json!({"target_triangles":2,"max_error":1.0}).to_string();
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/assets/{}/simplify", source.id))
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
        assert_eq!(submitted.state, meshharbor_core::JobState::Running);
        assert_eq!(
            submitted.started_at.as_deref(),
            Some(submitted.created_at.as_str())
        );

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
        let result = final_job.result.as_ref().unwrap();
        assert!(result["asset_id"].is_string());
        assert_eq!(result["report"]["triangles_after"], 2);
        assert_eq!(core.store().list_assets(10).unwrap().len(), 2);

        let step_request = |authenticated| {
            let mut request = axum::http::Request::builder()
                .uri(format!("/api/v1/jobs/{}/steps", final_job.id))
                .header(header::HOST, "localhost:7432");
            if authenticated {
                request = request.header(header::AUTHORIZATION, "Bearer token");
            }
            request.body(Body::empty()).unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(step_request(false))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let steps_response = app.clone().oneshot(step_request(true)).await.unwrap();
        assert_eq!(steps_response.status(), StatusCode::OK);
        assert_eq!(steps_response.headers()[header::CACHE_CONTROL], "no-store");
        let steps: Value = serde_json::from_slice(
            &steps_response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes(),
        )
        .unwrap();
        let steps = steps.as_array().unwrap();
        assert_eq!(steps[0]["state"], "running");
        assert!(steps.iter().all(|step| step["state"] != "queued"));
        assert!(steps.iter().any(|step| step["stage"] == "process_mesh"));
        assert_eq!(steps.last().unwrap()["state"], "succeeded");

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
            &listed_response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes(),
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
        let source = core.import_asset("source.png", &minimal_png()).unwrap();
        let app = api_router(
            AppState {
                core,
                scheduler: Arc::new(JobScheduler::default()),
            },
            "token".into(),
        );
        let body = json!({
            "input":{"kind":"image","asset_id":source.id},
            "parameters":{"backend_id":"pixal3d-mlx","resolution":1024}
        })
        .to_string();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/generation")
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
            "invalid request: resolution is fixed at 512 for pixal3d-mlx."
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_generate_requires_exactly_one_input() {
        for arguments in [
            vec!["meshharbor", "local", "generate", "asset_demo"],
            vec![
                "meshharbor",
                "local",
                "generate",
                "--prompt",
                "a ceramic cup",
            ],
            vec![
                "meshharbor",
                "local",
                "generate",
                "--prompt",
                "a ceramic cup",
                "--backend-id",
                "trellis-text-cuda",
            ],
        ] {
            assert!(Cli::try_parse_from(arguments).is_ok());
        }
        assert!(Cli::try_parse_from(["meshharbor", "local", "generate"]).is_err());
        assert!(
            Cli::try_parse_from([
                "meshharbor",
                "local",
                "generate",
                "asset_demo",
                "--prompt",
                "a cup"
            ])
            .is_err()
        );
    }

    #[tokio::test]
    async fn model_generation_rejects_unsupported_text_without_queuing_and_preserves_image_requests()
     {
        let root = std::env::temp_dir().join(format!(
            "meshharbor-model-generate-api-{}",
            uuid::Uuid::new_v4()
        ));
        let core = OperationCore::open(&root).unwrap();
        let source = core.import_asset("source.png", &minimal_png()).unwrap();
        let scheduler = Arc::new(JobScheduler::default());
        let permit = scheduler.try_acquire_owned().unwrap();
        let app = api_router(
            AppState {
                core: core.clone(),
                scheduler: scheduler.clone(),
            },
            "token".into(),
        );
        let submit = |input: Value, backend_id: &str| {
            app.clone().oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/generation")
                    .header(header::HOST, "localhost:7432")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "input": input,
                            "parameters": { "backend_id": backend_id },
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
        };
        for input in [
            json!({"kind":"text","prompt":"  "}),
            json!({"kind":"text","prompt":"가".repeat(1366)}),
            json!({"kind":"text","prompt":"a\u{0}cup"}),
        ] {
            assert_eq!(
                submit(input, "pixal3d-mlx").await.unwrap().status(),
                StatusCode::BAD_REQUEST
            );
        }
        let response = submit(
            json!({"kind":"text","prompt":"a private draft"}),
            "pixal3d-mlx",
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let error = response.into_body().collect().await.unwrap().to_bytes();
        let error = String::from_utf8_lossy(&error);
        assert!(error.contains("does not support text-to-3D"));
        assert!(!error.contains("a private draft"));
        let response = submit(
            json!({"kind":"text","prompt":"a private draft"}),
            "trellis-text-cuda",
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let error = response.into_body().collect().await.unwrap().to_bytes();
        let error = String::from_utf8_lossy(&error);
        assert!(error.contains("trellis-text-cuda is not ready"));
        assert!(!error.contains("a private draft"));
        assert!(core.store().list_jobs(10).unwrap().is_empty());
        assert_eq!(core.store().list_assets(10).unwrap().len(), 1);

        let mut jobs = Vec::new();
        for _ in 0..2 {
            let input = json!({"kind":"image","asset_id":source.id});
            let response = submit(input, "pixal3d-mlx").await.unwrap();
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            let job: meshharbor_core::Job =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            assert_eq!(job.operation, MODEL_GENERATION_OPERATION);
            assert_eq!(job.input_asset_id.as_deref(), Some(source.id.as_str()));
            assert_eq!(job.parameters["input_mode"], "image");
            jobs.push(job.id);
        }
        assert_eq!(scheduler.generation_queue(), jobs);
        for job in jobs {
            core.store().request_job_cancellation(&job, true).unwrap();
            scheduler.remove(&job);
        }
        drop(permit);
        drop(app);
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_output_refuses_existing_files_and_preserves_contents() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-cli-output-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let existing = root.join("existing.glb");
        let sentinel = b"do not overwrite";
        std::fs::write(&existing, sentinel).unwrap();

        let preflight = preflight_cli_output_paths(Some(&existing), None).unwrap_err();
        assert!(matches!(preflight, CoreError::Conflict(_)));
        let write = write_cli_output(&existing, b"replacement").unwrap_err();
        assert!(matches!(write, CoreError::Conflict(_)));
        assert_eq!(std::fs::read(&existing).unwrap(), sentinel);

        let new_path = root.join("new.glb");
        preflight_cli_output_paths(Some(&new_path), None).unwrap();
        let written_path = write_cli_output(&new_path, b"new output").unwrap();
        assert_eq!(
            written_path,
            std::fs::canonicalize(&root).unwrap().join("new.glb")
        );
        assert_eq!(std::fs::read(&new_path).unwrap(), b"new output");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cli_output_refuses_symlink_destinations_without_touching_targets() {
        use std::os::unix::fs::symlink;

        let root =
            std::env::temp_dir().join(format!("meshharbor-cli-symlink-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("protected.glb");
        let sentinel = b"protected data";
        std::fs::write(&target, sentinel).unwrap();
        let link = root.join("linked.glb");
        symlink(&target, &link).unwrap();

        let preflight = preflight_cli_output_paths(Some(&link), None).unwrap_err();
        assert!(matches!(preflight, CoreError::Conflict(_)));
        let write = write_cli_output(&link, b"replacement").unwrap_err();
        assert!(matches!(write, CoreError::Conflict(_)));
        assert_eq!(std::fs::read(&target).unwrap(), sentinel);
        assert_eq!(std::fs::read(&link).unwrap(), sentinel);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_mesh_output_paths_reject_lexical_aliases_before_processing() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-cli-pair-{}", uuid::Uuid::new_v4()));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let output = nested.join("../same.glb");
        let quad_output = root.join("same.glb");

        let error = preflight_cli_output_paths(Some(&output), Some(&quad_output)).unwrap_err();
        assert!(matches!(error, CoreError::InvalidInput(_)));
        assert!(!root.join("same.glb").exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cli_output_failure_preserves_a_replacement_destination() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-cli-failure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("failed.glb");
        let moved = root.join("original.partial");
        let replacement = b"replacement from a competing writer";

        let error = write_cli_output_with(&destination, b"original output", |file, path, bytes| {
            std::io::Write::write_all(file, &bytes[..8])?;
            std::fs::rename(path, &moved)?;
            std::fs::write(path, replacement)?;
            Err(std::io::Error::other("injected write failure"))
        })
        .unwrap_err();
        match error {
            CoreError::Io(error) => assert!(error.to_string().contains("may be incomplete")),
            other => panic!("expected an I/O error, got {other:?}"),
        }
        assert_eq!(std::fs::read(&destination).unwrap(), replacement);
        assert_eq!(std::fs::read(&moved).unwrap(), b"original");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn cli_output_paths_reject_windows_case_only_aliases() {
        let root = std::env::temp_dir().join(format!(
            "meshharbor-cli-case-alias-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let output = root.join("mesh.glb");
        let quad_output = root.join("MESH.GLB");

        let error = preflight_cli_output_paths(Some(&output), Some(&quad_output)).unwrap_err();
        assert!(matches!(error, CoreError::InvalidInput(_)));
        assert!(!output.exists());
        assert!(!quad_output.exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_output_creation_is_race_safe() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-cli-race-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("race.glb");
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let first_path = destination.clone();
        let first_barrier = Arc::clone(&barrier);
        let first = std::thread::spawn(move || {
            first_barrier.wait();
            match write_cli_output(&first_path, b"first") {
                Ok(_) => Ok(true),
                Err(CoreError::Conflict(_)) => Ok(false),
                Err(error) => Err(error.to_string()),
            }
        });
        let second_path = destination.clone();
        let second_barrier = Arc::clone(&barrier);
        let second = std::thread::spawn(move || {
            second_barrier.wait();
            match write_cli_output(&second_path, b"second") {
                Ok(_) => Ok(true),
                Err(CoreError::Conflict(_)) => Ok(false),
                Err(error) => Err(error.to_string()),
            }
        });

        barrier.wait();
        let results = [
            first.join().unwrap().unwrap(),
            second.join().unwrap().unwrap(),
        ];
        assert_eq!(results.iter().filter(|written| **written).count(), 1);
        let written = std::fs::read(&destination).unwrap();
        assert!(written == b"first" || written == b"second");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mcp_export_is_confined_to_the_configured_directory() {
        let root = tempfile::tempdir().unwrap();
        let export_root = root.path().join("exports");
        let target = export_root.join("result.glb");
        assert_eq!(
            canonical_mcp_export_path(root.path(), &target)
                .unwrap()
                .file_name(),
            target.file_name()
        );
        assert!(
            canonical_mcp_export_path(root.path(), std::path::Path::new("result.glb")).is_err()
        );
        assert!(canonical_mcp_export_path(root.path(), &root.path().join("escape.glb")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path(), export_root.join("linked")).unwrap();
            assert!(
                canonical_mcp_export_path(root.path(), &export_root.join("linked/result.glb"))
                    .is_err()
            );
        }
    }
}
