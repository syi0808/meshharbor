use std::{
    collections::VecDeque,
    env, fs,
    io::{self, Read, Write},
    net::SocketAddr,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use meshharbor_core::{CoreError, prepare_blender_glb};
use serde::{Deserialize, Serialize};
use tokio::{process::Command, task, time::timeout};
use uuid::Uuid;

use crate::{ApiError, AppState, data_dir_lock::DataDirLock};

const ADDON_FILENAME: &str = "meshharbor_bridge.py";
const ADDON_SOURCE: &str = include_str!("../blender/meshharbor_bridge.py");
const ADDON_MARKER: &str = "\"name\": \"MeshHarbor Bridge\"";
const PREFERENCES_FILENAME: &str = "userpref.blend";
const MAX_PREFERENCES_BYTES: u64 = 64 * 1024 * 1024;
const HEARTBEAT_LIMIT: Duration = Duration::from_secs(8);
pub(crate) const TRANSFER_LIMIT: Duration = Duration::from_secs(240);
const RECENT_TRANSFER_LIMIT: usize = 16;
static INSTALLATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const PROBE: &str = r#"import bpy, json; print("MESHHARBOR_BLENDER:" + json.dumps({"version": bpy.app.version, "addons": bpy.utils.user_resource("SCRIPTS", path="addons"), "config": bpy.utils.user_resource("CONFIG")}))"#;
const ACTIVATE: &str = r#"
import addon_utils
import bpy
import os
import sys
from pathlib import Path

assert Path(bpy.utils.user_resource("CONFIG")).resolve() == Path(os.environ["BLENDER_USER_CONFIG"]).resolve()
assert not bpy.context.preferences.app_template
module = "meshharbor_bridge"
enabled, loaded = addon_utils.check(module)
if not (enabled and loaded):
    assert bpy.ops.preferences.addon_enable(module=module) == {"FINISHED"}
    assert addon_utils.check(module) == (True, True)
assert Path(sys.modules[module].__file__).resolve() == Path(os.environ["MESHHARBOR_BLENDER_ADDON"]).resolve()
if enabled and loaded:
    print("MESHHARBOR_ALREADY_ENABLED")
else:
    assert bpy.ops.wm.save_userpref() == {"FINISHED"}
    print("MESHHARBOR_ACTIVATED")
"#;

#[path = "blender_processes.rs"]
mod processes;

#[derive(Clone)]
struct Bridge {
    listen: SocketAddr,
    queue: Arc<Mutex<Queue>>,
}

#[derive(Default)]
struct Queue {
    client: Option<(Uuid, Instant)>,
    transfer: Option<Transfer>,
    recent: VecDeque<SendStatus>,
}

struct Transfer {
    id: Uuid,
    client: Uuid,
    artifact: String,
    started: Instant,
    stage: Stage,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    Waiting,
    Importing,
    Succeeded,
    Failed,
}

impl Stage {
    pub(crate) fn active(self) -> bool {
        matches!(self, Self::Waiting | Self::Importing)
    }
}

impl Queue {
    fn refresh(&mut self) {
        if let Some(transfer) = self.transfer.as_mut()
            && transfer.stage.active()
            && (transfer.started.elapsed() >= TRANSFER_LIMIT
                || (transfer.stage == Stage::Waiting
                    && !self.client.is_some_and(|(client, last_seen)| {
                        client == transfer.client && last_seen.elapsed() < HEARTBEAT_LIMIT
                    })))
        {
            transfer.stage = Stage::Failed;
            transfer.error =
                Some("Blender did not finish the transfer. Check the bridge and try again.".into());
        }
    }

    fn connected_client(&self) -> Option<Uuid> {
        if let Some(transfer) = self.transfer.as_ref()
            && transfer.stage == Stage::Importing
        {
            return Some(transfer.client);
        }
        self.client
            .filter(|(_, last_seen)| last_seen.elapsed() < HEARTBEAT_LIMIT)
            .map(|(client, _)| client)
    }
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
pub(crate) struct SendRequest {
    pub(crate) asset_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub(crate) struct SendStatus {
    pub(crate) id: Uuid,
    pub(crate) stage: Stage,
    pub(crate) error: Option<String>,
}

impl From<&Transfer> for SendStatus {
    fn from(transfer: &Transfer) -> Self {
        Self {
            id: transfer.id,
            stage: transfer.stage,
            error: transfer.error.clone(),
        }
    }
}

#[derive(Deserialize)]
struct ClientPoll {
    client_id: Uuid,
}

#[derive(Deserialize)]
struct ClientResult {
    client_id: Uuid,
    error: Option<String>,
}

pub(crate) fn routes(listen: SocketAddr) -> Router<AppState> {
    let bridge = Bridge {
        listen,
        queue: Arc::default(),
    };
    Router::new()
        .route("/api/v1/blender", get(status))
        .route("/api/v1/blender/addon", get(download_addon))
        .route("/api/v1/blender/install", post(install_addon))
        .route("/api/v1/blender/send", post(send))
        .route("/api/v1/blender/send/{id}", get(send_status))
        .route("/api/v1/blender/client/poll", post(poll))
        .route("/api/v1/blender/client/disconnect", post(disconnect))
        .route("/api/v1/blender/client/result/{id}", post(complete))
        .layer(DefaultBodyLimit::max(4096))
        .layer(Extension(bridge))
}

async fn status(Extension(bridge): Extension<Bridge>) -> impl IntoResponse {
    let mut queue = bridge.queue.lock().unwrap();
    queue.refresh();
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({
            "connected": queue.connected_client().is_some(),
        })),
    )
}

async fn send(
    State(state): State<AppState>,
    Extension(bridge): Extension<Bridge>,
    Json(request): Json<SendRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let artifact = state
        .core
        .store()
        .get_asset(&request.asset_id)?
        .ok_or_else(|| CoreError::NotFound("This model version no longer exists.".into()))?
        .artifacts
        .into_iter()
        .find(|artifact| artifact.mime_type == "model/gltf-binary")
        .ok_or_else(|| CoreError::InvalidInput("Select a GLB model to send to Blender.".into()))?;
    let mut queue = bridge.queue.lock().unwrap();
    queue.refresh();
    let client = queue.connected_client().ok_or_else(|| {
        CoreError::Conflict(
            "Blender is not connected. Start the bridge in Blender's MeshHarbor panel.".into(),
        )
    })?;
    if queue
        .transfer
        .as_ref()
        .is_some_and(|transfer| transfer.stage.active())
    {
        return Err(
            CoreError::Conflict("Another transfer to Blender is in progress.".into()).into(),
        );
    }
    let transfer = Transfer {
        id: Uuid::new_v4(),
        client,
        artifact: artifact.id,
        started: Instant::now(),
        stage: Stage::Waiting,
        error: None,
    };
    let response = SendStatus::from(&transfer);
    if let Some(previous) = queue.transfer.replace(transfer) {
        if queue.recent.len() == RECENT_TRANSFER_LIMIT {
            queue.recent.pop_front();
        }
        queue.recent.push_back(SendStatus::from(&previous));
    }
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn send_status(
    Extension(bridge): Extension<Bridge>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let mut queue = bridge.queue.lock().unwrap();
    queue.refresh();
    let transfer = queue
        .transfer
        .as_ref()
        .filter(|transfer| transfer.id == id)
        .map(SendStatus::from)
        .or_else(|| {
            queue
                .recent
                .iter()
                .find(|transfer| transfer.id == id)
                .cloned()
        })
        .ok_or_else(|| {
            CoreError::NotFound("This Blender transfer is no longer available.".into())
        })?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(transfer)))
}

async fn poll(
    State(state): State<AppState>,
    Extension(bridge): Extension<Bridge>,
    Json(request): Json<ClientPoll>,
) -> Result<Response, ApiError> {
    let (id, artifact) = {
        let mut queue = bridge.queue.lock().unwrap();
        queue.refresh();
        if queue
            .connected_client()
            .is_some_and(|client| client != request.client_id)
        {
            return Err(CoreError::Conflict(
                "Another Blender instance is already connected.".into(),
            )
            .into());
        }
        queue.client = Some((request.client_id, Instant::now()));
        let Some(transfer) = queue
            .transfer
            .as_mut()
            .filter(|transfer| transfer.stage == Stage::Waiting)
        else {
            return Ok(StatusCode::NO_CONTENT.into_response());
        };
        transfer.stage = Stage::Importing;
        (transfer.id, transfer.artifact.clone())
    };
    let result = task::spawn_blocking(move || {
        prepare_blender_glb(state.core.store().read_artifact(&artifact)?.bytes)
    })
    .await
    .unwrap_or_else(|_| {
        Err(CoreError::OperationUnavailable(
            "Could not prepare the Blender transfer.".into(),
        ))
    });
    match result {
        Ok(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("model/gltf-binary"),
            );
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert(
                "x-meshharbor-transfer",
                HeaderValue::from_str(&id.to_string()).unwrap(),
            );
            Ok((headers, bytes).into_response())
        }
        Err(error) => {
            finish(&bridge, id, request.client_id, Some(error.to_string()))?;
            Err(error.into())
        }
    }
}

async fn complete(
    Extension(bridge): Extension<Bridge>,
    Path(id): Path<Uuid>,
    Json(result): Json<ClientResult>,
) -> Result<StatusCode, ApiError> {
    finish(&bridge, id, result.client_id, result.error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn disconnect(
    Extension(bridge): Extension<Bridge>,
    Json(request): Json<ClientPoll>,
) -> StatusCode {
    let mut queue = bridge.queue.lock().unwrap();
    if queue
        .client
        .is_some_and(|(client, _)| client == request.client_id)
    {
        queue.client = None;
        if let Some(transfer) = queue
            .transfer
            .as_mut()
            .filter(|transfer| transfer.stage.active())
        {
            transfer.stage = Stage::Failed;
            transfer.error = Some("The Blender bridge stopped before the import finished.".into());
        }
    }
    StatusCode::NO_CONTENT
}

fn finish(bridge: &Bridge, id: Uuid, client: Uuid, error: Option<String>) -> Result<(), CoreError> {
    let mut queue = bridge.queue.lock().unwrap();
    queue.refresh();
    let transfer = queue
        .transfer
        .as_mut()
        .filter(|transfer| {
            transfer.id == id && transfer.client == client && transfer.stage == Stage::Importing
        })
        .ok_or_else(|| CoreError::NotFound("This Blender transfer is no longer active.".into()))?;
    transfer.stage = if error.is_some() {
        Stage::Failed
    } else {
        Stage::Succeeded
    };
    transfer.error = error.map(|error| error.chars().take(500).collect());
    queue.client = Some((client, Instant::now()));
    Ok(())
}

fn addon_source(listen: SocketAddr) -> String {
    ADDON_SOURCE.replace("http://127.0.0.1:7432", &format!("http://{listen}"))
}

async fn download_addon(Extension(bridge): Extension<Bridge>) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"meshharbor_bridge.py\"",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        addon_source(bridge.listen),
    )
}

#[derive(Deserialize)]
struct BlenderLocation {
    version: [u16; 3],
    addons: PathBuf,
    config: PathBuf,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct InstallResult {
    version: String,
}

async fn install_addon(
    Extension(bridge): Extension<Bridge>,
) -> Result<Json<InstallResult>, ApiError> {
    Ok(Json(install_local(bridge.listen).await?))
}

pub(crate) async fn install_local(listen: SocketAddr) -> Result<InstallResult, CoreError> {
    if FsPath::new("/.dockerenv").exists() || FsPath::new("/run/.containerenv").exists() {
        return Err(CoreError::OperationUnavailable(
            "MeshHarbor is running in a container. Download the add-on and install it on the host from Blender.".into(),
        ));
    }
    let _installation = INSTALLATION.try_lock().map_err(|_| {
        CoreError::Conflict("Another Blender add-on installation is in progress.".into())
    })?;
    installation_io(ensure_blender_closed).await?;
    let (executable, location) = find_blender().await?;
    let [major, minor, patch] = location.version;
    let addon = location.addons.join(ADDON_FILENAME);
    let (staged, profile_lock) =
        installation_io(move || {
            ensure_blender_closed()?;
            let profile_lock = DataDirLock::try_acquire_for_blender_install(&location.config)
                .map_err(|error| match error.kind() {
                    io::ErrorKind::WouldBlock => CoreError::Conflict(
                        "Another Blender add-on installation is in progress.".into(),
                    ),
                    _ => error.into(),
                })?;
            ensure_blender_closed()?;
            let staged = StagedPreferences::open(&location.config)?;
            install_source(&location.addons, &addon_source(listen))?;
            Ok((staged, profile_lock))
        })
        .await?;
    let mut command = blender_command(executable);
    command
        .args([
            "--log",
            "blend,wm.files",
            "--log-level",
            if major >= 5 { "info" } else { "1" },
        ])
        .args(["--python-expr", ACTIVATE])
        .env("BLENDER_USER_CONFIG", staged.directory.path())
        .env("MESHHARBOR_BLENDER_ADDON", addon);
    let output = timeout(Duration::from_secs(25), command.output())
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or_else(activation_failed)?;
    let saved = output
        .stdout
        .split(|byte| *byte == b'\n')
        .any(|line| line.strip_suffix(b"\r").unwrap_or(line) == b"MESHHARBOR_ACTIVATED");
    let already_enabled = output
        .stdout
        .split(|byte| *byte == b'\n')
        .any(|line| line.strip_suffix(b"\r").unwrap_or(line) == b"MESHHARBOR_ALREADY_ENABLED");
    if !output.status.success()
        || (!saved && !already_enabled)
        || (staged.original.is_some()
            && !preferences_loaded(&output.stdout, &staged.staged_path())
            && !preferences_loaded(&output.stderr, &staged.staged_path()))
    {
        return Err(activation_failed());
    }
    installation_io(move || {
        let _profile_lock = profile_lock;
        if saved {
            staged.commit(ensure_blender_closed)
        } else {
            ensure_blender_closed()
        }
    })
    .await?;
    Ok(InstallResult {
        version: format!("{major}.{minor}.{patch}"),
    })
}

async fn installation_io<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, CoreError> + Send + 'static,
) -> Result<T, CoreError> {
    task::spawn_blocking(work).await.map_err(|_| {
        CoreError::OperationUnavailable("Could not install the Blender add-on.".into())
    })?
}

fn activation_failed() -> CoreError {
    CoreError::OperationUnavailable(
        "Could not safely activate the add-on. Blender preferences were not changed. Use Download add-on to install manually.".into(),
    )
}

fn ensure_blender_closed() -> Result<(), CoreError> {
    match processes::blender_running() {
        Ok(false) => Ok(()),
        Ok(true) => Err(CoreError::Conflict(
            "Blender is running. Quit Blender, then retry the add-on installation.".into(),
        )),
        Err(_) => Err(CoreError::OperationUnavailable(
            "Could not verify that Blender is closed. No Blender preferences were changed. Use Download add-on to install manually.".into(),
        )),
    }
}

fn preferences_loaded(log: &[u8], expected: &FsPath) -> bool {
    String::from_utf8_lossy(log).lines().any(|line| {
        line.split_once("Read prefs: \"")
            .or_else(|| line.split_once("read prefs: \""))
            .and_then(|(_, rest)| rest.strip_suffix('"'))
            .is_some_and(|path| FsPath::new(path) == expected)
    })
}

struct StagedPreferences {
    directory: tempfile::TempDir,
    destination: PathBuf,
    original: Option<Vec<u8>>,
}

impl StagedPreferences {
    fn open(config: &FsPath) -> Result<Self, CoreError> {
        fs::create_dir_all(config)?;
        let destination = config.join(PREFERENCES_FILENAME);
        let original = read_preferences(&destination)?;
        let directory = tempfile::Builder::new()
            .prefix(".meshharbor-")
            .tempdir_in(config)?;
        let staged = Self {
            directory,
            destination,
            original,
        };
        if let Some(original) = &staged.original {
            fs::write(staged.staged_path(), original)?;
        }
        Ok(staged)
    }

    fn staged_path(&self) -> PathBuf {
        self.directory.path().join(PREFERENCES_FILENAME)
    }

    fn commit(
        self,
        check_blender_closed: impl FnOnce() -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let staged = self.staged_path();
        if !read_preferences(&staged)?.is_some_and(|bytes| !bytes.is_empty()) {
            return Err(activation_failed());
        }
        if read_preferences(&self.destination)? != self.original {
            return Err(CoreError::Conflict(
                "Blender preferences changed during installation. Quit Blender and try again."
                    .into(),
            ));
        }
        fs::OpenOptions::new()
            .write(true)
            .open(&staged)?
            .sync_all()?;
        if self.original.is_some() {
            fs::set_permissions(&staged, fs::metadata(&self.destination)?.permissions())?;
        }
        check_blender_closed()?;
        fs::rename(staged, &self.destination)?;
        Ok(())
    }
}

fn read_preferences(path: &FsPath) -> Result<Option<Vec<u8>>, CoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_PREFERENCES_BYTES => {}
        Ok(_) => return Err(activation_failed()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_PREFERENCES_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PREFERENCES_BYTES {
        return Err(activation_failed());
    }
    Ok(Some(bytes))
}

fn blender_command(executable: PathBuf) -> Command {
    let mut command = Command::new(executable);
    command.args([
        "--background",
        "--disable-autoexec",
        "--python-exit-code",
        "1",
    ]);
    command.kill_on_drop(true);
    command.env_remove("MESHHARBOR_API_TOKEN");
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
}

async fn find_blender() -> Result<(PathBuf, BlenderLocation), CoreError> {
    for executable in blender_candidates().into_iter().take(12) {
        let mut command = blender_command(executable.clone());
        command.args(["--factory-startup", "--python-expr", PROBE]);
        if let Ok(Ok(output)) = timeout(Duration::from_secs(5), command.output()).await
            && output.status.success()
            && let Some(location) =
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find_map(|line| {
                        serde_json::from_str::<BlenderLocation>(
                            line.strip_prefix("MESHHARBOR_BLENDER:")?,
                        )
                        .ok()
                    })
            && location.version >= [4, 2, 0]
            && location.addons.is_absolute()
            && location.config.is_absolute()
        {
            return Ok((executable, location));
        }
    }
    Err(CoreError::OperationUnavailable(
        "Could not find Blender 4.2 or newer. Install Blender or download the add-on and use Install from Disk.".into(),
    ))
}

fn blender_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let executable = if cfg!(windows) {
        "blender.exe"
    } else {
        "blender"
    };
    let mut add = |path: PathBuf| {
        if path.is_absolute() && path.is_file() && !candidates.contains(&path) {
            candidates.push(path);
        }
    };
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            add(directory.join(executable));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut directories = vec![PathBuf::from("/Applications")];
        if let Some(home) = env::var_os("HOME") {
            directories.push(PathBuf::from(home).join("Applications"));
        }
        for directory in directories {
            add(directory.join("Blender.app/Contents/MacOS/Blender"));
            for bundle in installation_directories(&directory, "blender") {
                add(bundle.join("Contents/MacOS/Blender"));
            }
        }
    }
    #[cfg(windows)]
    for directory in ["ProgramFiles", "ProgramW6432"] {
        if let Some(base) = env::var_os(directory) {
            let base = PathBuf::from(base).join("Blender Foundation");
            for installation in installation_directories(&base, "blender") {
                add(installation.join(executable));
            }
        }
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    for path in [
        "/usr/bin/blender",
        "/usr/local/bin/blender",
        "/snap/bin/blender",
    ] {
        add(PathBuf::from(path));
    }
    candidates
}

#[cfg(any(target_os = "macos", windows))]
fn installation_directories(base: &FsPath, prefix: &str) -> Vec<PathBuf> {
    let mut installations: Vec<_> = fs::read_dir(base)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with(prefix)
        })
        .map(|entry| entry.path())
        .collect();
    installations.sort_unstable_by(|left, right| right.cmp(left));
    installations
}

fn install_source(directory: &FsPath, source: &str) -> Result<(), CoreError> {
    fs::create_dir_all(directory)?;
    let destination = directory.join(ADDON_FILENAME);
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= 64 * 1024
            && fs::read_to_string(&destination)?.contains(ADDON_MARKER) => {}
        Ok(_) => return Err(CoreError::Conflict(
            "A different file already uses the MeshHarbor add-on name. Move it or install from disk.".into(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let temporary = directory.join(format!(".meshharbor-{}.tmp", Uuid::new_v4()));
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(source.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use meshharbor_core::OperationCore;
    use serde_json::json;
    use tower::ServiceExt;

    #[tokio::test]
    async fn bridge_endpoint_requires_native_headers_and_installer_is_studio_only() {
        let root = env::temp_dir().join(format!("meshharbor-bridge-{}", Uuid::new_v4()));
        let app = crate::api_router(
            AppState {
                core: OperationCore::open(&root).unwrap(),
                scheduler: Arc::default(),
            },
            "token".into(),
        );
        let payload = || Body::from(json!({"client_id": Uuid::new_v4()}).to_string());
        for (extra_header, expected) in [
            (None, StatusCode::NO_CONTENT),
            (
                Some(("origin", "http://127.0.0.1:7432")),
                StatusCode::FORBIDDEN,
            ),
            (
                Some(("sec-fetch-site", "same-origin")),
                StatusCode::FORBIDDEN,
            ),
        ] {
            let mut request = Request::post("/api/v1/blender/client/poll")
                .header(header::HOST, "127.0.0.1:7432")
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-meshharbor-blender", "local-addon-v1");
            if let Some((key, value)) = extra_header {
                request = request.header(key, value);
            }
            let response = app
                .clone()
                .oneshot(request.body(payload()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/blender/install")
                    .header(header::HOST, "127.0.0.1:7432")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .oneshot(
                Request::get("/api/v1/assets")
                    .header(header::HOST, "127.0.0.1:7432")
                    .header("x-meshharbor-blender", "local-addon-v1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installer_only_replaces_its_own_addon() {
        let root = env::temp_dir().join(format!("meshharbor-addon-{}", Uuid::new_v4()));
        let source = addon_source("[::1]:8888".parse().unwrap());
        install_source(&root, &source).unwrap();
        let destination = root.join(ADDON_FILENAME);
        assert!(
            fs::read_to_string(&destination)
                .unwrap()
                .contains("http://[::1]:8888")
        );
        install_source(&root, &addon_source("127.0.0.1:7432".parse().unwrap())).unwrap();
        assert_eq!(fs::read_to_string(&destination).unwrap(), ADDON_SOURCE);
        fs::write(&destination, "other add-on").unwrap();
        assert!(matches!(
            install_source(&root, &source),
            Err(CoreError::Conflict(_))
        ));
        assert_eq!(fs::read_to_string(destination).unwrap(), "other add-on");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn activation_only_publishes_if_blender_is_closed_and_preferences_are_unchanged() {
        let config = tempfile::tempdir().unwrap();
        let destination = config.path().join(PREFERENCES_FILENAME);
        fs::write(&destination, b"original prefs").unwrap();

        let prepare = || {
            let staged = StagedPreferences::open(config.path()).unwrap();
            assert_eq!(fs::read(staged.staged_path()).unwrap(), b"original prefs");
            fs::write(staged.staged_path(), b"updated prefs").unwrap();
            staged
        };
        assert!(matches!(
            prepare().commit(|| Err(CoreError::Conflict("Blender is running.".into()))),
            Err(CoreError::Conflict(_)),
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"original prefs");

        let staged = prepare();
        fs::write(&destination, b"new preferences from Blender").unwrap();
        assert!(matches!(
            staged.commit(|| Ok(())),
            Err(CoreError::Conflict(_))
        ));
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"new preferences from Blender"
        );

        fs::write(&destination, b"original prefs").unwrap();
        prepare().commit(|| Ok(())).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"updated prefs");
        assert_eq!(fs::read_dir(config.path()).unwrap().count(), 1);
    }

    #[test]
    fn reads_preference_load_confirmation_from_supported_blender_logs() {
        let path = FsPath::new("staging folder").join(PREFERENCES_FILENAME);
        for line in [
            "INFO (wm.files): read prefs:",
            "00:00.123 blend | Read prefs:",
        ] {
            assert!(preferences_loaded(
                format!("{line} \"{}\"\n", path.display()).as_bytes(),
                &path
            ));
        }
        assert!(!preferences_loaded(
            b"read prefs: \"different/userpref.blend\"",
            &path
        ));
        assert!(!preferences_loaded(
            b"Read prefs from app-template: \"staging folder/userpref.blend\"",
            &path
        ));
    }
}
