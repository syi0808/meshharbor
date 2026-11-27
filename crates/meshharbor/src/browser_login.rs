use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use meshharbor_core::{CoreError, HuggingFaceBrowserTool, Pixal3dProfile};
use serde::Serialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    sync::{Mutex, watch},
    task,
    time::timeout,
};
use uuid::Uuid;

use crate::huggingface_status::HuggingFaceStatuses;
use crate::{ApiError, AppState, acquire_runtime_write_lock};

#[derive(Clone, Default)]
pub(crate) struct BrowserLoginSessions(Arc<Mutex<Option<Session>>>);

struct Session {
    backend: String,
    view: LoginView,
    cancel: watch::Sender<bool>,
}

#[derive(Clone, Serialize, schemars::JsonSchema)]
pub(crate) struct LoginView {
    id: Uuid,
    #[serde(flatten)]
    phase: LoginPhase,
}

#[derive(Clone, Serialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
enum LoginPhase {
    Preparing,
    AwaitingAuthorization { user_code: String, expires_in: u64 },
    SignedIn,
    Cancelled,
    Expired,
    Failed { message: String },
}

impl LoginPhase {
    fn is_active(&self) -> bool {
        matches!(self, Self::Preparing | Self::AwaitingAuthorization { .. })
    }
}

pub(crate) async fn start(
    State(state): State<AppState>,
    Extension(sessions): Extension<BrowserLoginSessions>,
    Extension(statuses): Extension<HuggingFaceStatuses>,
    Path(backend): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let profile = Pixal3dProfile::from_backend(&backend)?;
    let response = |view| {
        (
            StatusCode::ACCEPTED,
            [(header::CACHE_CONTROL, "no-store")],
            Json(view),
        )
    };
    let mut current = sessions.0.lock().await;
    if let Some(session) = current
        .as_ref()
        .filter(|session| session.view.phase.is_active())
    {
        if session.backend == backend {
            return Ok(response(session.view.clone()));
        }
        return Err(CoreError::Conflict(
            "Finish or cancel the current Hugging Face sign-in first.".into(),
        )
        .into());
    }
    let id = Uuid::new_v4();
    let (cancel, mut cancellation) = watch::channel(false);
    let view = LoginView {
        id,
        phase: LoginPhase::Preparing,
    };
    *current = Some(Session {
        backend,
        view: view.clone(),
        cancel,
    });
    drop(current);
    let root = state.core.store().root().to_path_buf();
    task::spawn(async move {
        let phase = tokio::select! {
            biased;
            _ = cancellation.changed() => LoginPhase::Cancelled,
            result = run_login(root, profile, &sessions, id, statuses) => match result {
                Ok(phase) => phase,
                Err(error) => LoginPhase::Failed { message: public_message(error) },
            }
        };
        sessions.set_phase(id, phase).await;
    });
    Ok(response(view))
}

pub(crate) async fn status(
    Extension(sessions): Extension<BrowserLoginSessions>,
    Path((backend, id)): Path<(String, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let current = sessions.0.lock().await;
    let session = find_session(&current, &backend, id)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(session.view.clone()),
    ))
}

pub(crate) async fn cancel(
    Extension(sessions): Extension<BrowserLoginSessions>,
    Path((backend, id)): Path<(String, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let current = sessions.0.lock().await;
    let session = find_session(&current, &backend, id)?;
    if session.view.phase.is_active() {
        let _ = session.cancel.send(true);
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(session.view.clone()),
    ))
}

fn find_session<'a>(
    slot: &'a Option<Session>,
    backend: &str,
    id: Uuid,
) -> Result<&'a Session, CoreError> {
    slot.as_ref()
        .filter(|session| session.backend == backend && session.view.id == id)
        .ok_or_else(|| {
            CoreError::NotFound("Hugging Face sign-in session not found. Start again.".into())
        })
}

impl BrowserLoginSessions {
    async fn set_phase(&self, id: Uuid, phase: LoginPhase) {
        if let Some(current) = self
            .0
            .lock()
            .await
            .as_mut()
            .filter(|session| session.view.id == id)
        {
            current.view.phase = phase;
        }
    }
}

async fn run_login(
    root: PathBuf,
    profile: Pixal3dProfile,
    sessions: &BrowserLoginSessions,
    id: Uuid,
    statuses: HuggingFaceStatuses,
) -> Result<LoginPhase, CoreError> {
    let status_root = root.clone();
    let (runtime_lock, tool) = task::spawn_blocking(move || {
        let runtime_lock = acquire_runtime_write_lock(&root)?;
        let tool = HuggingFaceBrowserTool::new(profile, &root)?;
        Ok::<_, CoreError>((runtime_lock, tool))
    })
    .await
    .map_err(|_| login_error())??;
    let _runtime_lock = runtime_lock;
    let credentials = tool.stage()?;
    let mut child = tool
        .login_command(&credentials)
        .spawn()
        .map_err(|_| login_error())?;
    let stdout = child.stdout.take().expect("piped Hugging Face CLI stdout");
    let mut bounded = BufReader::new(stdout).take(4096);
    let mut prompt = String::new();
    timeout(Duration::from_secs(30), bounded.read_line(&mut prompt))
        .await
        .map_err(|_| login_error())?
        .map_err(|_| login_error())?;
    let (user_code, expires_in) = parse_prompt(&prompt).ok_or_else(login_error)?;
    sessions
        .set_phase(
            id,
            LoginPhase::AwaitingAuthorization {
                user_code,
                expires_in,
            },
        )
        .await;
    let started = std::time::Instant::now();
    let lifetime = Duration::from_secs(expires_in);
    match timeout(lifetime, child.wait()).await {
        Err(_) => return Ok(LoginPhase::Expired),
        Ok(Ok(status)) if status.success() => {}
        Ok(_) if started.elapsed() >= lifetime.saturating_sub(Duration::from_secs(2)) => {
            return Ok(LoginPhase::Expired);
        }
        Ok(_) => return Err(login_error()),
    }
    tool.commit(credentials)?;
    statuses.signed_in(&status_root, profile);
    Ok(LoginPhase::SignedIn)
}

fn parse_prompt(line: &str) -> Option<(String, u64)> {
    let (url, instructions) = line
        .strip_prefix("Ask the user to open ")?
        .split_once(" in a browser and enter the code ")?;
    if ![
        "https://hf.co/oauth/device",
        "https://huggingface.co/oauth/device",
    ]
    .iter()
    .any(|base| {
        url.strip_prefix(base)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('?'))
    }) {
        return None;
    }
    let (code, expiry) = instructions.split_once(". The code expires in ")?;
    let (first, second) = code.split_once('-')?;
    if ![first, second].iter().all(|part| {
        part.len() == 4
            && part
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    }) {
        return None;
    }
    let expires_in = expiry
        .strip_suffix(" seconds. Waiting for authorization...\n")?
        .parse()
        .ok()?;
    if !(1..=1800).contains(&expires_in) {
        return None;
    }
    Some((code.into(), expires_in))
}

fn login_error() -> CoreError {
    CoreError::OperationUnavailable(
        "Hugging Face browser sign-in did not complete. Try again, or use an access token.".into(),
    )
}

fn public_message(error: CoreError) -> String {
    match error {
        CoreError::OperationUnavailable(message) | CoreError::Conflict(message) => message,
        _ => login_error().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_prompt;

    #[test]
    fn extracts_only_code_and_lifetime_from_the_pinned_cli_prompt() {
        let prompt = |url: &str, code: &str, expiry: &str| {
            format!(
                "Ask the user to open {url} in a browser and enter the code {code}. The code expires in {expiry} seconds. Waiting for authorization...\n"
            )
        };
        for url in [
            "https://hf.co/oauth/device",
            "https://huggingface.co/oauth/device?user_code=ABCD-EFG1",
        ] {
            assert_eq!(
                parse_prompt(&prompt(url, "ABCD-EFG1", "300")),
                Some(("ABCD-EFG1".into(), 300))
            );
        }
        for (url, code, expiry) in [
            (
                "https://hf.co.evil.example/oauth/device",
                "ABCD-EFG1",
                "300",
            ),
            (
                "https://huggingface.co/oauth/device/other",
                "ABCD-EFG1",
                "300",
            ),
            ("https://hf.co/oauth/device", "hf_secret", "300"),
            ("https://hf.co/oauth/device", "ABCD-EFG1", "1801"),
        ] {
            assert_eq!(parse_prompt(&prompt(url, code, expiry)), None);
        }
    }
}
