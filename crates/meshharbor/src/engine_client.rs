use std::{io, net::SocketAddr, path::Path, time::Duration};

use clap::{Subcommand, ValueEnum};
use reqwest::{Method, Url, header};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ApiErrorBody, DEFAULT_LISTEN, MAX_ASSET_BYTES};

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

fn parse_json(value: &str) -> Result<Value, String> {
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
    /// Search and page through stored assets or one current version per model.
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
        #[arg(long)]
        #[serde(default)]
        models_only: bool,
    },
    /// Read one stored asset version and its artifacts.
    Show { asset_id: String },
    /// List a model's version history and current version.
    Versions {
        model_id: String,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        before: Option<u64>,
    },
    /// Set an existing version as current without deleting any versions.
    Restore { model_id: String, asset_id: String },
    /// Save the exact bytes of an artifact to a new local file.
    Export {
        artifact_id: String,
        /// New file; for MCP, an absolute path inside MESHHARBOR_EXPORT_DIR.
        output_path: String,
    },
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
    /// Read the persisted job timeline and redacted command logs.
    Steps { job_id: String },
    /// Submit an asset operation to the same queue as Studio and return its job ID.
    Submit {
        operation: String,
        /// REST job input (for example {"kind":"image","asset_id":"asset_..."}).
        #[arg(long = "input", value_parser = parse_json)]
        input: Value,
        /// REST job parameters. Supports generation tuning and multiview references.
        #[arg(long, value_parser = parse_json)]
        parameters: Option<Value>,
    },
    /// Start a new attempt of a replayable failed or stopped job.
    Retry {
        job_id: String,
        /// Required; a replayed runtime job may download model files.
        #[arg(long)]
        #[serde(default)]
        confirmed: bool,
    },
    /// Stop a job, or remove it only if it is still queued.
    Cancel {
        job_id: String,
        #[arg(long)]
        #[serde(default)]
        queued_only: bool,
    },
    /// List waiting generation jobs in execution order.
    Queue,
    /// Swap two adjacent waiting generation jobs.
    Swap { job_id: String, swap_with: String },
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PixalProfile {
    Pixal3dMlx,
    Pixal3dCuda,
}

impl PixalProfile {
    fn backend(self) -> &'static str {
        match self {
            Self::Pixal3dMlx => "pixal3d-mlx",
            Self::Pixal3dCuda => "pixal3d-cuda",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Subcommand)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Runtime {
    /// Check that the running server is reachable.
    Health,
    /// Show the server's model catalog, readiness, and cached account/access status.
    Status,
    /// List the implemented and unavailable engine operations.
    Operations,
    /// List recent setup and install jobs for one backend.
    Jobs { backend: String },
    /// Set up shared Python, uv, and account tools. Does not install weights.
    SetupEnvironment {
        #[arg(long)]
        #[serde(default)]
        confirmed: bool,
    },
    /// Install a catalog model's backend and weights after owner approval.
    Install {
        backend: String,
        #[arg(long)]
        #[serde(default)]
        confirmed: bool,
        #[arg(long)]
        #[serde(default)]
        confirm_gated_access: bool,
    },
    /// Remove managed model files while preserving assets and sign-in.
    RemoveModelFiles {
        backend: String,
        #[arg(long)]
        #[serde(default)]
        confirmed: bool,
    },
    /// Set up the selected Pixal3D runtime separately from weights.
    SetupPixal3d {
        #[arg(value_enum)]
        backend: PixalProfile,
    },
    /// Download Pixal3D model files after large-download and gated-access confirmation.
    InstallPixal3dModels {
        #[arg(value_enum)]
        backend: PixalProfile,
        #[arg(long)]
        #[serde(default)]
        confirm_large_download: bool,
        #[arg(long)]
        #[serde(default)]
        confirm_gated_access: bool,
    },
    /// Convert existing Pixal3D MLX NAF weights.
    ConvertNaf,
    /// Download the additional pinned Pixal3D CUDA multiview weights.
    InstallMultiview {
        #[arg(long)]
        #[serde(default)]
        confirm_large_download: bool,
    },
    /// Recheck the saved Hugging Face account without accepting any secret.
    CheckAccount {
        #[arg(value_enum)]
        backend: PixalProfile,
    },
    /// Recheck saved DINOv3 model access.
    CheckModelAccess {
        #[arg(value_enum)]
        backend: PixalProfile,
    },
    /// Start a Hugging Face device flow that the owner completes in their browser.
    LoginStart {
        #[arg(value_enum)]
        backend: PixalProfile,
    },
    /// Read device session state, including the one-time code for the owner.
    LoginStatus {
        #[arg(value_enum)]
        backend: PixalProfile,
        #[schemars(with = "String")]
        session: Uuid,
    },
    /// Cancel the pending device sign-in session.
    LoginCancel {
        #[arg(value_enum)]
        backend: PixalProfile,
        #[schemars(with = "String")]
        session: Uuid,
    },
    /// Remove saved local Hugging Face credentials, not the remote token or weights.
    Disconnect {
        #[arg(value_enum)]
        backend: PixalProfile,
        #[arg(long)]
        #[serde(default)]
        confirmed: bool,
    },
}

#[derive(Debug, Deserialize, JsonSchema, Subcommand)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Blender {
    /// Check whether a Blender bridge is currently connected.
    Status,
    /// Save the bundled .py add-on for manual installation (does not install it).
    DownloadAddon {
        /// New .py file; for MCP, an absolute path inside MESHHARBOR_EXPORT_DIR.
        output_path: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Browse, restore, and export stored models and assets.
    Assets {
        #[command(subcommand)]
        action: Assets,
    },
    /// Manage the live Studio job queue and persisted job history.
    Jobs {
        #[command(subcommand)]
        action: Jobs,
    },
    /// Manage model runtimes and connections on the running server.
    Runtime {
        #[command(subcommand)]
        action: Runtime,
    },
    /// Check bridge connection status or download its bundled add-on.
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
            Self::Jobs { action } => client.jobs(action).await,
            Self::Runtime { action } => client.runtime(action).await,
            Self::Blender { action } => client.blender(action, None).await,
        }
    }
}

pub(crate) struct Client {
    base: Url,
    http: reqwest::Client,
}

impl Client {
    pub(crate) fn configured() -> io::Result<Self> {
        let server = match std::env::var(SERVER_ENV) {
            Ok(value) => parse_server(&value),
            Err(std::env::VarError::NotPresent) => parse_server(DEFAULT_LISTEN),
            Err(std::env::VarError::NotUnicode(_)) => Err("MESHHARBOR_SERVER must be valid text."),
        }
        .map_err(io::Error::other)?;
        Self::new(server)
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
                models_only,
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
                    &["assets", "page"],
                    &[
                        ("limit", limit.map(|value| value.to_string())),
                        ("cursor", cursor),
                        ("q", query),
                        ("kind", kind),
                        ("derived", derived.map(|value| value.to_string())),
                        ("models_only", Some(models_only.to_string())),
                    ],
                    None,
                )
                .await
            }
            Assets::Show { asset_id } => self.get(&["assets", &asset_id]).await,
            Assets::Versions {
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
            Assets::Restore { model_id, asset_id } => {
                self.json(
                    Method::PUT,
                    &["models", &model_id, "current"],
                    &[],
                    Some(json!({"asset_id": asset_id})),
                )
                .await
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
            Jobs::Submit {
                operation,
                input,
                parameters,
            } => {
                self.post(
                    &["jobs"],
                    Some(json!({"operation": operation, "input": input, "parameters": parameters})),
                )
                .await
            }
            Jobs::Retry { job_id, confirmed } => {
                if !confirmed {
                    return Err(io::Error::other(
                        "Confirm the retry before starting a new attempt.",
                    )
                    .into());
                }
                self.post(&["jobs", &job_id, "retry"], None).await
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
            Jobs::Queue => self.get(&["jobs", "generation-queue"]).await,
            Jobs::Swap { job_id, swap_with } => {
                self.post(
                    &["jobs", "generation-queue"],
                    Some(json!({"job_id": job_id, "swap_with": swap_with})),
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
            Runtime::SetupEnvironment { confirmed } => self.post(&["runtime", "environment", "setup"], Some(json!({"confirmed": confirmed}))).await,
            Runtime::Install { backend, confirmed, confirm_gated_access } => self.post(&["runtime", &backend, "install"], Some(json!({"confirmed": confirmed, "confirm_gated_access": confirm_gated_access}))).await,
            Runtime::RemoveModelFiles { backend, confirmed } => self.json(Method::DELETE, &["runtime", &backend, "model-files"], &[], Some(json!({"confirmed": confirmed}))).await,
            Runtime::SetupPixal3d { backend } => self.post(&["runtime", backend.backend(), "setup"], None).await,
            Runtime::InstallPixal3dModels { backend, confirm_large_download, confirm_gated_access } => self.post(&["runtime", backend.backend(), "models"], Some(json!({"confirm_large_download": confirm_large_download, "confirm_gated_access": confirm_gated_access}))).await,
            Runtime::ConvertNaf => self.post(&["runtime", "pixal3d-mlx", "convert-naf"], None).await,
            Runtime::InstallMultiview { confirm_large_download } => self.post(&["runtime", "pixal3d-cuda", "multiview"], Some(json!({"confirm_large_download": confirm_large_download}))).await,
            Runtime::CheckAccount { backend } => self.post(&["runtime", backend.backend(), "huggingface", "check"], None).await,
            Runtime::CheckModelAccess { backend } => self.post(&["runtime", backend.backend(), "model-access"], None).await,
            Runtime::LoginStart { backend } => self.post(&["runtime", backend.backend(), "huggingface", "browser"], None).await,
            Runtime::LoginStatus { backend, session } => self.get(&["runtime", backend.backend(), "huggingface", "browser", &session.to_string()]).await,
            Runtime::LoginCancel { backend, session } => self.json(Method::DELETE, &["runtime", backend.backend(), "huggingface", "browser", &session.to_string()], &[], None).await,
            Runtime::Disconnect { backend, confirmed } => self.json(Method::DELETE, &["runtime", backend.backend(), "huggingface"], &[], Some(json!({"confirmed": confirmed}))).await,
        }
    }

    pub(crate) async fn blender(
        &self,
        action: Blender,
        mcp_data_dir: Option<&Path>,
    ) -> Result<Value, crate::BoxError> {
        match action {
            Blender::Status => self.get(&["blender"]).await,
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
