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
use meshharbor_core::{CoreError, OperationCore};
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

#[derive(Debug, Parser)]
#[command(
    name = "meshharbor",
    version,
    about = "Local-first 3D asset processing core"
)]
struct Cli {
    #[arg(long, env = "MESHHARBOR_HOME", default_value_os_t = default_data_dir())]
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
    /// Show implemented operations and detected runtime capabilities.
    Doctor,
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
            let core = OperationCore::open(&root)?;
            let token = std::env::var("MESHHARBOR_API_TOKEN")
                .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
            let app = api_router(
                AppState {
                    core,
                    scheduler: Arc::new(Semaphore::new(1)),
                },
                token.clone(),
            );
            let listener = TcpListener::bind(listen).await?;
            eprintln!("MeshHarbor REST API listening at http://{listen}; bearer token: {token}");
            axum::serve(listener, app).await?;
        }
        Command::Mcp => {
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
        Command::Doctor => println!(
            "{}",
            serde_json::to_string_pretty(&OperationCore::open(&root)?.runtime_doctor())?
        ),
    }
    Ok(())
}

fn api_router(state: AppState, token: String) -> Router {
    Router::new()
        .route("/api/v1/health", get(api_health))
        .route("/api/v1/runtime", get(api_runtime))
        .route("/api/v1/operations", get(api_operations))
        .route(
            "/api/v1/assets",
            get(api_list_assets).post(api_import_asset),
        )
        .route("/api/v1/assets/{asset_id}", get(api_get_asset))
        .route("/api/v1/assets/{asset_id}/simplify", post(api_simplify))
        .route("/api/v1/jobs", post(api_create_job))
        .route("/api/v1/jobs/{job_id}", get(api_get_job))
        .route("/api/v1/jobs/{job_id}/cancel", post(api_cancel_job))
        .route("/api/v1/artifacts/{artifact_id}", get(api_read_artifact))
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .fallback(api_studio_fallback)
        .layer(middleware::from_fn_with_state(token, authenticate))
        .with_state(state)
}

async fn authenticate(State(token): State<String>, request: Request, next: Next) -> Response {
    let host_ok = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(is_loopback_authority);
    let origin_ok = request
        .headers()
        .get(header::ORIGIN)
        .map(|value| value.to_str().ok().is_some_and(is_loopback_origin))
        .unwrap_or(true);
    if !host_ok || !origin_ok {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Only loopback Host and Origin values are accepted."})),
        )
            .into_response();
    }
    let path = request.uri().path();
    if !is_api_path(path) || path == "/api/v1/health" {
        return next.run(request).await;
    }
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|provided| constant_time_equal(provided.as_bytes(), token.as_bytes()));
    if authorized {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"A valid bearer token is required."})),
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
            return response.into_response();
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
    (headers, body).into_response()
}

async fn api_health() -> Json<Value> {
    Json(json!({"status":"ok","service":"meshharbor"}))
}
async fn api_runtime(State(state): State<AppState>) -> Json<meshharbor_core::RuntimeDoctor> {
    Json(state.core.runtime_doctor())
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
    parameters: SimplifyBody,
}

#[derive(Debug, Deserialize)]
struct JobStartInput {
    asset_id: String,
}

async fn api_create_job(
    State(state): State<AppState>,
    Json(body): Json<JobStartBody>,
) -> Result<(StatusCode, Json<meshharbor_core::Job>), ApiError> {
    if body.operation != "mesh.simplify" {
        return Err(ApiError(CoreError::OperationUnavailable(format!(
            "Operation '{}' is not implemented by this build.",
            body.operation
        ))));
    }
    let job = start_simplify_job(
        state.core,
        state.scheduler,
        &body.input.asset_id,
        body.parameters.target_triangles,
        body.parameters.max_error,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
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

fn default_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("MESHHARBOR_HOME") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("MeshHarbor");
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path).join("Library/Application Support/MeshHarbor");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("meshharbor");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".local/share/meshharbor")
}

fn default_max_error() -> f32 {
    0.02
}
fn default_limit() -> usize {
    50
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
    let authority = authority.trim();
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

fn is_loopback_origin(origin: &str) -> bool {
    let authority = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .map(|rest| rest.split('/').next().unwrap_or_default());
    authority.is_some_and(is_loopback_authority)
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

    #[test]
    fn validates_base64_padding() {
        assert_eq!(decode_base64("dGVzdA==").unwrap(), b"test");
        assert!(decode_base64("test===").is_err());
        assert!(decode_base64("dGV$").is_err());
    }

    #[test]
    fn api_token_comparison_checks_full_value() {
        assert!(constant_time_equal(b"a token", b"a token"));
        assert!(!constant_time_equal(b"a token", b"a tokeN"));
        assert!(!constant_time_equal(b"a", b"aa"));
    }

    #[tokio::test]
    async fn rest_api_allows_health_and_requires_token_for_asset_routes() {
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

        let upload = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/assets?filename=sample.png")
                    .header(header::HOST, "localhost:7432")
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
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }
}
