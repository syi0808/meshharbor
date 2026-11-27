use std::{io, net::SocketAddr, path::Path, time::Duration};

use clap::{Subcommand, ValueEnum};
use reqwest::{Method, Url, header};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use meshharbor_core::{GenerationInput, GenerationOptions};

use crate::{ApiErrorBody, DEFAULT_LISTEN, MAX_ASSET_BYTES, actions};

pub(crate) const SERVER_ENV: &str = "MESHHARBOR_SERVER";
const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 16 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request<Action> {
    pub(crate) request: Action,
}

pub(crate) fn parse_server(value: &str) -> Result<SocketAddr, &'static str> {
    value
        .parse::<SocketAddr>()
        .ok()
        .filter(|address| address.ip().is_loopback() && address.port() != 0)
        .ok_or("The engine must use a loopback IP and port, such as 127.0.0.1:7432.")
}

fn parse_generation_options(value: &str) -> Result<GenerationOptions, String> {
    serde_json::from_str(value).map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AssetKind {
    Image,
    Mesh,
}

#[derive(Debug, Deserialize, JsonSchema, Subcommand)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Assets {
    /// Search and page through images and mesh versions.
    Page {
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long, value_enum)]
        kind: Option<AssetKind>,
        #[arg(long, value_name = "BOOL")]
        derived: Option<bool>,
    },
    /// Read one version and its artifact metadata.
    Show { asset_id: String },
    /// Upload a PNG, JPEG, or GLB to the running engine.
    #[serde(skip)]
    #[schemars(skip)]
    Import { file: std::path::PathBuf },
    /// Download an artifact to a new file. MCP destinations must be in MESHHARBOR_EXPORT_DIR.
    Export {
        artifact_id: String,
        output_path: String,
    },
}

#[derive(Debug, Deserialize, JsonSchema, Subcommand)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Models {
    /// List one current version per model.
    List {
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long, value_name = "BOOL")]
        generated: Option<bool>,
    },
    /// List versions and the current selection.
    Versions {
        model_id: String,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        before: Option<u64>,
    },
    /// Make an existing version current without deleting any versions.
    Restore { model_id: String, asset_id: String },
}

#[derive(Debug, Deserialize, JsonSchema, Subcommand)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Mesh {
    Simplify {
        asset_id: String,
        #[command(flatten)]
        options: actions::SimplifyMesh,
    },
    Cleanup {
        asset_id: String,
        #[command(flatten)]
        #[serde(default)]
        options: actions::CleanupMesh,
    },
    VoxelRemesh {
        asset_id: String,
        #[command(flatten)]
        #[serde(default)]
        options: actions::VoxelRemesh,
    },
    UnwrapUv {
        asset_id: String,
        #[command(flatten)]
        #[serde(default)]
        options: actions::UnwrapUv,
    },
    OptimizeRendering {
        asset_id: String,
        #[command(flatten)]
        #[serde(default)]
        options: actions::OptimizeRendering,
    },
    Compress {
        asset_id: String,
    },
    Retopology {
        asset_id: String,
        #[arg(long)]
        target_quads: Option<u32>,
    },
    #[command(name = "retopology-lato2")]
    RetopologyLato2 {
        asset_id: String,
        #[command(flatten)]
        #[serde(default)]
        options: actions::RetopologizeLato2,
    },
    Preflight {
        asset_id: String,
        #[arg(long)]
        target_quads: Option<u32>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum Generation {
    /// Generate from a stored image, optionally with additional posed views.
    Image {
        asset_id: String,
        #[arg(long)]
        backend: Option<String>,
        #[arg(long, value_parser = parse_generation_options)]
        parameters: Option<GenerationOptions>,
    },
    /// Generate from a text prompt.
    Text {
        prompt: String,
        #[arg(long)]
        backend: Option<String>,
        #[arg(long, value_parser = parse_generation_options)]
        parameters: Option<GenerationOptions>,
    },
    /// List waiting generation jobs in execution order.
    Queue,
    /// Swap two adjacent waiting generation jobs.
    Swap { job_id: String, swap_with: String },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum GenerationQueue {
    List,
    Swap { job_id: String, swap_with: String },
}

#[derive(Debug, Deserialize, JsonSchema, Subcommand)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Jobs {
    /// List persisted jobs, optionally including all active jobs and paging older records.
    List {
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        #[serde(default)]
        include_active: bool,
        #[arg(long)]
        before: Option<String>,
    },
    /// Read one job.
    Show { job_id: String },
    /// Read the persisted timeline and redacted command logs.
    Steps { job_id: String },
    /// Start a new attempt with the recorded inputs and options.
    Retry {
        job_id: String,
        /// Required when retrying a job that can download weights.
        #[arg(long)]
        #[serde(default)]
        confirm_download: bool,
    },
    /// Stop a job, or remove it only if it is still queued.
    Cancel {
        job_id: String,
        #[arg(long)]
        #[serde(default)]
        queued_only: bool,
    },
}

#[derive(Debug, Deserialize, JsonSchema, Subcommand)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Runtime {
    /// Check that the server is reachable.
    Health,
    /// Read the runtime catalog and readiness.
    Status,
    /// List operation capabilities.
    Operations,
    /// List recent setup and install jobs for one backend.
    Jobs { backend: String },
    /// Install the shared Python, uv, and account tools, without weights.
    SetupEnvironment {
        #[arg(long)]
        #[serde(default)]
        confirmed: bool,
    },
    /// Install a backend and its weights, or an advertised optional package.
    Install {
        backend: String,
        #[arg(long)]
        #[serde(default)]
        confirmed: bool,
        #[arg(long)]
        #[serde(default)]
        confirm_gated_access: bool,
        #[arg(long, value_enum)]
        package: Option<actions::RuntimePackage>,
    },
    /// Remove managed files without deleting credentials or assets.
    RemoveModelFiles {
        backend: String,
        #[arg(long)]
        #[serde(default)]
        confirmed: bool,
    },
    /// Check the guarded QuadriFlow runtime.
    Quadriflow,
    /// Install the verified QuadriFlow sidecar.
    InstallQuadriflow,
}

#[derive(Debug, Deserialize, JsonSchema, Subcommand)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Connections {
    CheckAccount {
        #[arg(value_enum)]
        scope: actions::AccountScope,
    },
    CheckModelAccess {
        #[arg(value_enum)]
        scope: actions::AccountScope,
    },
    LoginStart {
        #[arg(value_enum)]
        scope: actions::AccountScope,
    },
    LoginStatus {
        #[arg(value_enum)]
        scope: actions::AccountScope,
        session: Uuid,
    },
    LoginCancel {
        #[arg(value_enum)]
        scope: actions::AccountScope,
        session: Uuid,
    },
    Disconnect {
        #[arg(value_enum)]
        scope: actions::AccountScope,
        #[arg(long)]
        #[serde(default)]
        confirmed: bool,
    },
}

#[derive(Debug, Deserialize, JsonSchema, Subcommand)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Blender {
    Status,
    Send { asset_id: String },
    Transfer { transfer_id: Uuid },
    DownloadAddon { output_path: String },
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Import, browse, and export stored assets.
    Assets {
        #[command(subcommand)]
        action: Assets,
    },
    /// Browse and restore 3D model versions.
    Models {
        #[command(subcommand)]
        action: Models,
    },
    /// Edit or check a mesh version.
    Mesh {
        #[command(subcommand)]
        action: Mesh,
    },
    /// Generate 3D models and manage the generation queue.
    Generation {
        #[command(subcommand)]
        action: Generation,
    },
    /// Follow and control existing jobs.
    Jobs {
        #[command(subcommand)]
        action: Jobs,
    },
    /// Manage the engine environment and installed backends.
    Runtime {
        #[command(subcommand)]
        action: Runtime,
    },
    /// Verify or authorize a Hugging Face account.
    Connections {
        #[command(subcommand)]
        action: Connections,
    },
    /// Send a model to an open Blender scene.
    Blender {
        #[command(subcommand)]
        action: Blender,
    },
}

impl Command {
    pub(crate) async fn run(self, server: SocketAddr) -> Result<Value, crate::BoxError> {
        let client = Client::new(server)?;
        match self {
            Self::Assets { action } => client.assets(action, None).await,
            Self::Models { action } => client.models(action).await,
            Self::Mesh { action } => client.mesh(action).await,
            Self::Generation { action } => client.generation(action).await,
            Self::Jobs { action } => client.jobs(action).await,
            Self::Runtime { action } => client.runtime(action).await,
            Self::Connections { action } => client.connections(action).await,
            Self::Blender { action } => client.blender(action, None).await,
        }
    }
}

pub(crate) struct Client {
    base: Url,
    http: reqwest::Client,
}

pub(crate) fn configured_address() -> io::Result<SocketAddr> {
    match std::env::var(SERVER_ENV) {
        Ok(value) => parse_server(&value),
        Err(std::env::VarError::NotPresent) => parse_server(DEFAULT_LISTEN),
        Err(std::env::VarError::NotUnicode(_)) => Err("MESHHARBOR_SERVER must be valid text."),
    }
    .map_err(io::Error::other)
}

impl Client {
    pub(crate) fn configured() -> io::Result<Self> {
        Self::new(configured_address()?)
    }

    fn new(server: SocketAddr) -> io::Result<Self> {
        if !server.ip().is_loopback() || server.port() == 0 {
            return Err(io::Error::other("The engine must use a loopback address."));
        }
        let mut headers = header::HeaderMap::new();
        match std::env::var("MESHHARBOR_API_TOKEN") {
            Ok(token) if !token.trim().is_empty() => {
                let mut bearer = header::HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| {
                        io::Error::other("MESHHARBOR_API_TOKEN is not a valid bearer token.")
                    })?;
                bearer.set_sensitive(true);
                headers.insert(header::AUTHORIZATION, bearer);
            }
            Ok(_) | Err(std::env::VarError::NotUnicode(_)) => {
                return Err(io::Error::other(
                    "MESHHARBOR_API_TOKEN is not a valid bearer token.",
                ));
            }
            Err(std::env::VarError::NotPresent) => {}
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(60))
            .default_headers(headers)
            .build()
            .map_err(|_| io::Error::other("Could not initialize the MeshHarbor engine client."))?;
        let base = Url::parse(&format!("http://{server}/api/v1/"))
            .map_err(|_| io::Error::other("Invalid MeshHarbor engine address."))?;
        Ok(Self { base, http })
    }

    fn url(&self, path: &[&str]) -> io::Result<Url> {
        let mut url = self.base.clone();
        let mut segments = url.path_segments_mut().expect("loopback HTTP base URL");
        segments.pop_if_empty();
        for segment in path {
            if segment.is_empty() || matches!(*segment, "." | "..") || segment.contains(['/', '\\'])
            {
                return Err(io::Error::other("Invalid MeshHarbor identifier."));
            }
            segments.push(segment);
        }
        drop(segments);
        Ok(url)
    }

    async fn json(
        &self,
        method: Method,
        path: &[&str],
        query: &[(&str, Option<String>)],
        body: Option<Value>,
    ) -> Result<Value, crate::BoxError> {
        let mut url = self.url(path)?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(
                query
                    .iter()
                    .filter_map(|(key, value)| Some((*key, value.as_deref()?))),
            );
        }
        let mut request = self.http.request(method, url);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let reply = self.send(request).await?;
        let bytes = read_response(reply, MAX_JSON_BYTES).await?;
        serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::other("The MeshHarbor server returned invalid JSON.").into())
    }

    async fn get(&self, path: &[&str]) -> Result<Value, crate::BoxError> {
        self.json(Method::GET, path, &[], None).await
    }

    async fn post(&self, path: &[&str], body: Option<Value>) -> Result<Value, crate::BoxError> {
        self.json(Method::POST, path, &[], body).await
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> io::Result<reqwest::Response> {
        let reply = request.send().await.map_err(|error| {
            if error.is_connect() {
                io::Error::other(format!(
                    "Start MeshHarbor at {} first.",
                    self.base.origin().ascii_serialization()
                ))
            } else {
                io::Error::other("Could not reach the MeshHarbor engine API.")
            }
        })?;
        let status = reply.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Set MESHHARBOR_API_TOKEN to the running MeshHarbor server's bearer token.",
            ));
        }
        if !status.is_success() {
            let bytes = read_response(reply, MAX_ERROR_BYTES).await?;
            let message = serde_json::from_slice::<ApiErrorBody>(&bytes)
                .map(|response| response.error)
                .unwrap_or_else(|_| format!("MeshHarbor returned HTTP {status}."));
            return Err(io::Error::other(message));
        }
        Ok(reply)
    }

    async fn download(
        &self,
        path: &[&str],
        output: &Path,
        mcp_data_dir: Option<&Path>,
        limit: usize,
    ) -> Result<Value, crate::BoxError> {
        let output = match mcp_data_dir {
            Some(root) => crate::canonical_mcp_export_path(root, output)?,
            None => crate::canonical_cli_output_path(output)?,
        };
        crate::preflight_cli_output_paths(Some(&output), None)?;
        let reply = self.send(self.http.get(self.url(path)?)).await?;
        let media_type = reply
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = read_response(reply, limit).await?;
        let byte_size = bytes.len();
        let destination =
            tokio::task::spawn_blocking(move || crate::write_cli_output(&output, &bytes)).await??;
        Ok(
            json!({"output_path": destination, "byte_size": byte_size, "mime_type": media_type, "overwritten": false}),
        )
    }

    pub(crate) async fn assets(
        &self,
        action: Assets,
        mcp_data_dir: Option<&Path>,
    ) -> Result<Value, crate::BoxError> {
        match action {
            Assets::Page {
                limit,
                cursor,
                query,
                kind,
                derived,
            } => {
                let kind = kind.map(|kind| {
                    match kind {
                        AssetKind::Image => "image",
                        AssetKind::Mesh => "mesh",
                    }
                    .to_owned()
                });
                self.json(
                    Method::GET,
                    &["assets"],
                    &[
                        ("limit", limit.map(|value| value.to_string())),
                        ("cursor", cursor),
                        ("q", query),
                        ("kind", kind),
                        ("derived", derived.map(|value| value.to_string())),
                    ],
                    None,
                )
                .await
            }
            Assets::Show { asset_id } => self.get(&["assets", &asset_id]).await,
            Assets::Import { file } => {
                let bytes = crate::read_bounded_asset(&file, MAX_ASSET_BYTES).await?;
                let filename = file
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| io::Error::other("Use a file with a UTF-8 name."))?;
                self.import_asset(filename, bytes).await
            }
            Assets::Export {
                artifact_id,
                output_path,
            } => {
                self.download(
                    &["artifacts", &artifact_id],
                    Path::new(&output_path),
                    mcp_data_dir,
                    MAX_ASSET_BYTES,
                )
                .await
            }
        }
    }

    pub(crate) async fn import_asset(
        &self,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<Value, crate::BoxError> {
        let mut url = self.url(&["assets"])?;
        url.query_pairs_mut().append_pair("filename", filename);
        let reply = self
            .send(
                self.http
                    .post(url)
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(bytes),
            )
            .await?;
        Ok(serde_json::from_slice(
            &read_response(reply, MAX_JSON_BYTES).await?,
        )?)
    }

    pub(crate) async fn models(&self, action: Models) -> Result<Value, crate::BoxError> {
        match action {
            Models::List {
                limit,
                cursor,
                query,
                generated,
            } => {
                self.json(
                    Method::GET,
                    &["models"],
                    &[
                        ("limit", limit.map(|value| value.to_string())),
                        ("cursor", cursor),
                        ("q", query),
                        ("generated", generated.map(|value| value.to_string())),
                    ],
                    None,
                )
                .await
            }
            Models::Versions {
                model_id,
                limit,
                before,
            } => {
                self.json(
                    Method::GET,
                    &["models", &model_id, "versions"],
                    &[
                        ("limit", limit.map(|value| value.to_string())),
                        ("before", before.map(|value| value.to_string())),
                    ],
                    None,
                )
                .await
            }
            Models::Restore { model_id, asset_id } => {
                self.json(
                    Method::PUT,
                    &["models", &model_id, "current"],
                    &[],
                    Some(json!({"asset_id": asset_id})),
                )
                .await
            }
        }
    }

    pub(crate) async fn mesh(&self, action: Mesh) -> Result<Value, crate::BoxError> {
        let (asset_id, path, body) = match action {
            Mesh::Simplify { asset_id, options } => {
                (asset_id, "simplify", serde_json::to_value(options)?)
            }
            Mesh::Cleanup { asset_id, options } => {
                (asset_id, "cleanup", serde_json::to_value(options)?)
            }
            Mesh::VoxelRemesh { asset_id, options } => {
                (asset_id, "voxel-remesh", serde_json::to_value(options)?)
            }
            Mesh::UnwrapUv { asset_id, options } => {
                (asset_id, "unwrap-uv", serde_json::to_value(options)?)
            }
            Mesh::OptimizeRendering { asset_id, options } => (
                asset_id,
                "optimize-rendering",
                serde_json::to_value(options)?,
            ),
            Mesh::Compress { asset_id } => (asset_id, "compress", json!({})),
            Mesh::Retopology {
                asset_id,
                target_quads,
            } => (
                asset_id,
                "retopology",
                json!({"target_quads": target_quads}),
            ),
            Mesh::RetopologyLato2 { asset_id, options } => {
                (asset_id, "retopology-lato2", serde_json::to_value(options)?)
            }
            Mesh::Preflight {
                asset_id,
                target_quads,
            } => (
                asset_id,
                "retopology-preflight",
                json!({"target_quads": target_quads}),
            ),
        };
        self.post(&["assets", &asset_id, path], Some(body)).await
    }

    pub(crate) async fn generate(
        &self,
        action: actions::GenerateModel,
    ) -> Result<Value, crate::BoxError> {
        self.post(&["generation"], Some(serde_json::to_value(action)?))
            .await
    }

    pub(crate) async fn generation_queue(
        &self,
        action: GenerationQueue,
    ) -> Result<Value, crate::BoxError> {
        match action {
            GenerationQueue::List => self.get(&["generation", "queue"]).await,
            GenerationQueue::Swap { job_id, swap_with } => {
                self.post(
                    &["generation", "queue"],
                    Some(json!({"job_id": job_id, "swap_with": swap_with})),
                )
                .await
            }
        }
    }

    async fn generation(&self, action: Generation) -> Result<Value, crate::BoxError> {
        let (input, backend, parameters) = match action {
            Generation::Image {
                asset_id,
                backend,
                parameters,
            } => (GenerationInput::Image { asset_id }, backend, parameters),
            Generation::Text {
                prompt,
                backend,
                parameters,
            } => (GenerationInput::Text { prompt }, backend, parameters),
            Generation::Queue => return self.generation_queue(GenerationQueue::List).await,
            Generation::Swap { job_id, swap_with } => {
                return self
                    .generation_queue(GenerationQueue::Swap { job_id, swap_with })
                    .await;
            }
        };
        let mut parameters = parameters.unwrap_or_default();
        if let Some(backend) = backend {
            parameters.backend_id = backend;
        }
        self.generate(actions::GenerateModel { input, parameters })
            .await
    }

    pub(crate) async fn jobs(&self, action: Jobs) -> Result<Value, crate::BoxError> {
        match action {
            Jobs::List {
                limit,
                include_active,
                before,
            } => {
                self.json(
                    Method::GET,
                    &["jobs"],
                    &[
                        ("limit", limit.map(|value| value.to_string())),
                        ("include_active", Some(include_active.to_string())),
                        ("before", before),
                    ],
                    None,
                )
                .await
            }
            Jobs::Show { job_id } => self.get(&["jobs", &job_id]).await,
            Jobs::Steps { job_id } => self.get(&["jobs", &job_id, "steps"]).await,
            Jobs::Retry {
                job_id,
                confirm_download,
            } => {
                self.post(
                    &["jobs", &job_id, "retry"],
                    Some(json!({"confirm_download": confirm_download})),
                )
                .await
            }
            Jobs::Cancel {
                job_id,
                queued_only,
            } => {
                self.json(
                    Method::POST,
                    &["jobs", &job_id, "cancel"],
                    &[("queued_only", Some(queued_only.to_string()))],
                    None,
                )
                .await
            }
        }
    }

    pub(crate) async fn runtime(&self, action: Runtime) -> Result<Value, crate::BoxError> {
        match action {
            Runtime::Health => self.get(&["health"]).await,
            Runtime::Status => self.get(&["runtime"]).await,
            Runtime::Operations => self.get(&["operations"]).await,
            Runtime::Jobs { backend } => self.get(&["runtime", &backend, "jobs"]).await,
            Runtime::SetupEnvironment { confirmed } => {
                self.post(
                    &["runtime", "environment", "setup"],
                    Some(json!({"confirmed": confirmed})),
                )
                .await
            }
            Runtime::Install {
                backend,
                confirmed,
                confirm_gated_access,
                package,
            } => {
                self.post(
                    &["runtime", &backend, "install"],
                    Some(serde_json::to_value(actions::InstallBackend {
                        confirmed,
                        confirm_gated_access,
                        package: package.unwrap_or_default(),
                    })?),
                )
                .await
            }
            Runtime::RemoveModelFiles { backend, confirmed } => {
                self.json(
                    Method::DELETE,
                    &["runtime", &backend, "model-files"],
                    &[],
                    Some(json!({"confirmed": confirmed})),
                )
                .await
            }
            Runtime::Quadriflow => self.get(&["runtime", "quadriflow"]).await,
            Runtime::InstallQuadriflow => {
                self.post(&["runtime", "quadriflow", "install"], None).await
            }
        }
    }

    pub(crate) async fn connections(&self, action: Connections) -> Result<Value, crate::BoxError> {
        let prefix = ["connections", "huggingface"];
        match action {
            Connections::CheckAccount { scope } => {
                self.post(&[prefix[0], prefix[1], scope.segment(), "check"], None)
                    .await
            }
            Connections::CheckModelAccess { scope } => {
                self.post(
                    &[prefix[0], prefix[1], scope.segment(), "model-access"],
                    None,
                )
                .await
            }
            Connections::LoginStart { scope } => {
                self.post(&[prefix[0], prefix[1], scope.segment(), "sessions"], None)
                    .await
            }
            Connections::LoginStatus { scope, session } => {
                self.get(&[
                    prefix[0],
                    prefix[1],
                    scope.segment(),
                    "sessions",
                    &session.to_string(),
                ])
                .await
            }
            Connections::LoginCancel { scope, session } => {
                self.json(
                    Method::DELETE,
                    &[
                        prefix[0],
                        prefix[1],
                        scope.segment(),
                        "sessions",
                        &session.to_string(),
                    ],
                    &[],
                    None,
                )
                .await
            }
            Connections::Disconnect { scope, confirmed } => {
                self.json(
                    Method::DELETE,
                    &[prefix[0], prefix[1], scope.segment()],
                    &[],
                    Some(json!({"confirmed": confirmed})),
                )
                .await
            }
        }
    }

    pub(crate) async fn blender(
        &self,
        action: Blender,
        mcp_data_dir: Option<&Path>,
    ) -> Result<Value, crate::BoxError> {
        match action {
            Blender::Status => self.get(&["blender"]).await,
            Blender::Send { asset_id } => {
                self.post(&["blender", "send"], Some(json!({"asset_id": asset_id})))
                    .await
            }
            Blender::Transfer { transfer_id } => {
                self.get(&["blender", "send", &transfer_id.to_string()])
                    .await
            }
            Blender::DownloadAddon { output_path } => {
                let output = Path::new(&output_path);
                if !output
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("py"))
                {
                    return Err(io::Error::other(
                        "The Blender add-on must be saved as a .py file.",
                    )
                    .into());
                }
                self.download(&["blender", "addon"], output, mcp_data_dir, 1024 * 1024)
                    .await
            }
        }
    }
}

async fn read_response(mut reply: reqwest::Response, limit: usize) -> io::Result<Vec<u8>> {
    let too_large = || io::Error::other("The MeshHarbor response exceeds the size limit.");
    if reply.content_length().is_some_and(|len| len > limit as u64) {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = reply
        .chunk()
        .await
        .map_err(|_| io::Error::other("Could not read the MeshHarbor response."))?
    {
        if chunk.len() > limit - bytes.len() {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
