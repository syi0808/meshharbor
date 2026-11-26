use std::{
    env, fs,
    io::{self, Write},
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

use crate::{ApiError, AppState};

const ADDON_FILENAME: &str = "meshharbor_bridge.py";
const ADDON_SOURCE: &str = include_str!("../blender/meshharbor_bridge.py");
const ADDON_MARKER: &str = "\"name\": \"MeshHarbor Bridge\"";
const HEARTBEAT_LIMIT: Duration = Duration::from_secs(8);
const TRANSFER_LIMIT: Duration = Duration::from_secs(240);
const PROBE: &str = r#"import bpy, json; print("MESHHARBOR_BLENDER:" + json.dumps({"version": bpy.app.version, "addons": bpy.utils.user_resource("SCRIPTS", path="addons")}))"#;

#[derive(Clone)]
struct Bridge {
    listen: SocketAddr,
    queue: Arc<Mutex<Queue>>,
}

#[derive(Default)]
struct Queue {
    client: Option<(Uuid, Instant)>,
    transfer: Option<Transfer>,
}

struct Transfer {
    id: Uuid,
    client: Uuid,
    artifact: String,
    started: Instant,
    stage: Stage,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Stage {
    Waiting,
    Importing,
    Succeeded,
    Failed,
}

impl Stage {
    fn active(self) -> bool {
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

#[derive(Deserialize)]
struct SendRequest {
    asset_id: String,
}

#[derive(Serialize)]
struct SendStatus {
    id: Uuid,
    stage: Stage,
    error: Option<String>,
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
    queue.transfer = Some(transfer);
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
        .ok_or_else(|| {
            CoreError::NotFound("This Blender transfer is no longer available.".into())
        })?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(SendStatus::from(transfer)),
    ))
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
}

#[derive(Serialize)]
struct InstallResult {
    version: String,
}

async fn install_addon(
    Extension(bridge): Extension<Bridge>,
) -> Result<Json<InstallResult>, ApiError> {
    if FsPath::new("/.dockerenv").exists() || FsPath::new("/run/.containerenv").exists() {
        return Err(CoreError::OperationUnavailable(
            "MeshHarbor is running in a container. Download the add-on and install it on the host from Blender.".into(),
        ).into());
    }
    let location = find_blender().await?;
    task::spawn_blocking(move || {
        install_source(&location.addons, &addon_source(bridge.listen))?;
        let [major, minor, patch] = location.version;
        Ok(Json(InstallResult {
            version: format!("{major}.{minor}.{patch}"),
        }))
    })
    .await
    .map_err(|_| CoreError::OperationUnavailable("Could not install the Blender add-on.".into()))?
}

async fn find_blender() -> Result<BlenderLocation, CoreError> {
    for executable in blender_candidates().into_iter().take(12) {
        let mut command = Command::new(executable);
        command.args([
            "--background",
            "--factory-startup",
            "--disable-autoexec",
            "--python-exit-code",
            "1",
            "--python-expr",
            PROBE,
        ]);
        command.kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
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
        {
            return Ok(location);
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
}
