use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use meshharbor_core::{
    CoreResult, HuggingFaceAccess, HuggingFaceConnection, Pixal3dCudaRuntimeManager,
    Pixal3dProfile, Pixal3dRuntimeManager, RuntimeDoctor,
};
use serde::Serialize;
use serde_json::json;
use tokio::task;

#[derive(Clone, Copy, Serialize)]
struct Status {
    account: &'static str,
    model_access: &'static str,
}

impl Status {
    const CHECKING: Self = Self {
        account: "checking",
        model_access: "checking",
    };
    const NOT_CONNECTED: Self = Self {
        account: "not_connected",
        model_access: "not_connected",
    };
}

#[derive(Clone)]
pub(crate) struct HuggingFaceStatuses(Arc<Mutex<[(Status, u64); 2]>>);

impl Default for HuggingFaceStatuses {
    fn default() -> Self {
        Self(Arc::new(Mutex::new([(Status::CHECKING, 0); 2])))
    }
}

impl HuggingFaceStatuses {
    pub(crate) fn start(&self, root: &Path) {
        let statuses = self.clone();
        let root = root.to_path_buf();
        task::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let root = root.clone();
                let statuses = statuses.clone();
                let _ = task::spawn_blocking(move || {
                    for profile in [Pixal3dProfile::Mlx, Pixal3dProfile::Cuda] {
                        statuses.refresh(&root, profile);
                    }
                })
                .await;
            }
        });
    }

    pub(crate) fn decorate(&self, runtime: &mut RuntimeDoctor) {
        let statuses = self.0.lock().expect("Hugging Face status mutex poisoned");
        for backend in &mut runtime.model_backends {
            if let Some(profile) = backend["id"]
                .as_str()
                .and_then(|id| Pixal3dProfile::from_backend(id).ok())
            {
                backend["huggingface"] = json!(statuses[index(profile)].0);
            }
        }
    }

    fn refresh(&self, root: &Path, profile: Pixal3dProfile) {
        let revision = self.0.lock().expect("Hugging Face status mutex poisoned")[index(profile)].1;
        let has_token = match profile {
            Pixal3dProfile::Mlx => Pixal3dRuntimeManager::new(root).home_path(),
            Pixal3dProfile::Cuda => Pixal3dCudaRuntimeManager::new(root).home_path(),
        }
        .join(".cache/huggingface/token")
        .is_file();
        let status = if has_token {
            account_status(
                root,
                profile,
                check_connection(root, profile, None).as_ref().ok(),
            )
        } else {
            Status::NOT_CONNECTED
        };
        let mut statuses = self.0.lock().expect("Hugging Face status mutex poisoned");
        if statuses[index(profile)].1 == revision {
            statuses[index(profile)].0 = status;
        }
    }

    pub(crate) fn disconnected(&self, profile: Pixal3dProfile) {
        self.replace(profile, Status::NOT_CONNECTED);
    }

    pub(crate) fn signed_in(&self, root: &Path, profile: Pixal3dProfile) {
        let revision = self.replace(
            profile,
            Status {
                account: "connected",
                model_access: "checking",
            },
        );
        let statuses = self.clone();
        let root = root.to_path_buf();
        task::spawn_blocking(move || {
            let access = check_access(&root, profile);
            let mut statuses = statuses
                .0
                .lock()
                .expect("Hugging Face status mutex poisoned");
            let (cached, current_revision) = &mut statuses[index(profile)];
            if *current_revision == revision {
                cached.model_access = access_label(access.as_ref().ok());
            }
        });
    }

    pub(crate) fn record_account(
        &self,
        root: &Path,
        profile: Pixal3dProfile,
        account: Option<&HuggingFaceConnection>,
    ) {
        self.replace(profile, account_status(root, profile, account));
    }

    fn replace(&self, profile: Pixal3dProfile, status: Status) -> u64 {
        let mut statuses = self.0.lock().expect("Hugging Face status mutex poisoned");
        let (cached, revision) = &mut statuses[index(profile)];
        *cached = status;
        *revision += 1;
        *revision
    }

    pub(crate) fn record_access(
        &self,
        profile: Pixal3dProfile,
        access: Option<&HuggingFaceAccess>,
    ) {
        let mut statuses = self.0.lock().expect("Hugging Face status mutex poisoned");
        let (cached, revision) = &mut statuses[index(profile)];
        cached.model_access = access_label(access);
        *revision += 1;
    }
}

fn account_status(
    root: &Path,
    profile: Pixal3dProfile,
    account: Option<&HuggingFaceConnection>,
) -> Status {
    match account {
        Some(HuggingFaceConnection::Connected) => Status {
            account: "connected",
            model_access: access_label(check_access(root, profile).as_ref().ok()),
        },
        Some(HuggingFaceConnection::NotConnected) => Status::NOT_CONNECTED,
        Some(HuggingFaceConnection::InvalidToken) => Status {
            account: "invalid_token",
            model_access: "not_connected",
        },
        None => Status {
            account: "unavailable",
            model_access: "unavailable",
        },
    }
}

fn index(profile: Pixal3dProfile) -> usize {
    match profile {
        Pixal3dProfile::Mlx => 0,
        Pixal3dProfile::Cuda => 1,
    }
}

fn access_label(access: Option<&HuggingFaceAccess>) -> &'static str {
    match access {
        Some(HuggingFaceAccess::Verified) => "verified",
        Some(HuggingFaceAccess::AccessDenied) => "access_denied",
        Some(HuggingFaceAccess::NotConnected) => "not_connected",
        None => "unavailable",
    }
}

pub(crate) fn check_connection(
    root: &Path,
    profile: Pixal3dProfile,
    token: Option<&str>,
) -> CoreResult<HuggingFaceConnection> {
    match profile {
        Pixal3dProfile::Mlx => Pixal3dRuntimeManager::new(root).huggingface_connection(token),
        Pixal3dProfile::Cuda => Pixal3dCudaRuntimeManager::new(root).huggingface_connection(token),
    }
}

pub(crate) fn check_access(root: &Path, profile: Pixal3dProfile) -> CoreResult<HuggingFaceAccess> {
    match profile {
        Pixal3dProfile::Mlx => Pixal3dRuntimeManager::new(root).huggingface_model_access(),
        Pixal3dProfile::Cuda => Pixal3dCudaRuntimeManager::new(root).huggingface_model_access(),
    }
}
