use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{CoreError, CoreResult};

#[derive(Clone, Copy)]
pub enum Pixal3dProfile {
    Mlx,
    Cuda,
}

impl Pixal3dProfile {
    pub fn from_backend(backend: &str) -> CoreResult<Self> {
        match backend {
            "pixal3d-mlx" => Ok(Self::Mlx),
            "pixal3d-cuda" => Ok(Self::Cuda),
            _ => Err(CoreError::InvalidInput("Unknown Pixal3D backend.".into())),
        }
    }

    pub(crate) fn backend(self) -> &'static str {
        match self {
            Self::Mlx => "pixal3d-mlx",
            Self::Cuda => "pixal3d-cuda",
        }
    }

    fn home_overridden(self) -> bool {
        std::env::var_os(match self {
            Self::Mlx => "MESHHARBOR_PIXAL3D_MLX_HOME",
            Self::Cuda => "MESHHARBOR_PIXAL3D_CUDA_HOME",
        })
        .is_some()
    }

    fn credential_entries(self, root: &Path) -> CoreResult<Vec<LocalEntry>> {
        self.entries(root, CREDENTIAL_FILES)
    }

    pub(crate) fn ensure_managed_credentials(self, root: &Path) -> CoreResult<()> {
        self.credential_entries(root).map(drop)
    }

    fn model_entries(self, root: &Path) -> CoreResult<Vec<LocalEntry>> {
        if matches!(self, Self::Mlx) && std::env::var_os("MESHHARBOR_PIXAL3D_WEIGHTS_DIR").is_some()
        {
            return Err(non_managed_path());
        }
        let profile_entries = match self {
            Self::Mlx => MLX_MODELS,
            Self::Cuda => CUDA_MODELS,
        };
        self.entries(root, COMMON_MODELS.iter().chain(profile_entries))
    }

    fn entries<'a>(
        self,
        root: &Path,
        targets: impl IntoIterator<Item = &'a (&'a str, EntryKind)>,
    ) -> CoreResult<Vec<LocalEntry>> {
        if self.home_overridden() {
            return Err(non_managed_path());
        }
        let root = fs::canonicalize(root)?;
        targets
            .into_iter()
            .filter_map(|(relative, kind)| {
                let parts = ["runtimes", self.backend()]
                    .into_iter()
                    .chain(relative.split('/'));
                inspect_entry(&root, parts, *kind).transpose()
            })
            .collect()
    }

    pub(crate) fn local_files(self, root: &Path) -> RuntimeLocalFiles {
        let credentials = self.credential_entries(root);
        let models = self.model_entries(root);
        RuntimeLocalFiles {
            token_saved: credentials.as_ref().is_ok_and(|entries| {
                entries.iter().any(|entry| {
                    entry.path.file_name().is_some_and(|name| name == "token") && entry.size > 0
                })
            }),
            disconnect_available: credentials
                .as_ref()
                .is_ok_and(|entries| !entries.is_empty()),
            remove_models_available: models.as_ref().is_ok_and(|entries| !entries.is_empty()),
            credentials_managed: credentials.is_ok(),
            model_files_managed: models.is_ok(),
        }
    }

    pub fn disconnect_huggingface(self, root: &Path) -> CoreResult<()> {
        let entries = self.credential_entries(root)?;
        let hf_home = root
            .join("runtimes")
            .join(self.backend())
            .join("home/.cache/huggingface");
        crate::huggingface_browser::clear_staged_credentials(&hf_home)?;
        remove_entries(entries)
    }

    pub fn remove_models(self, root: &Path) -> CoreResult<()> {
        remove_entries(self.model_entries(root)?)
    }
}

#[derive(Serialize)]
pub(crate) struct RuntimeLocalFiles {
    token_saved: bool,
    disconnect_available: bool,
    remove_models_available: bool,
    credentials_managed: bool,
    model_files_managed: bool,
}

pub(crate) fn managed_directory(root: &Path, relative: &str) -> CoreResult<bool> {
    Ok(inspect_entry(&fs::canonicalize(root)?, relative.split('/'), Directory)?.is_some())
}

pub(crate) fn managed_file_size(root: &Path, relative: &str) -> CoreResult<Option<u64>> {
    Ok(inspect_entry(&fs::canonicalize(root)?, relative.split('/'), File)?.map(|entry| entry.size))
}

pub(crate) fn model_path(path: PathBuf) -> PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

pub(crate) fn remove_managed_directories(root: &Path, relative: &[&str]) -> CoreResult<()> {
    let root = fs::canonicalize(root)?;
    let entries = relative
        .iter()
        .filter_map(|path| inspect_entry(&root, path.split('/'), Directory).transpose())
        .collect::<CoreResult<_>>()?;
    remove_entries(entries)
}

#[derive(Clone, Copy)]
enum EntryKind {
    Directory,
    File,
}

use EntryKind::{Directory, File};

const CREDENTIAL_FILES: &[(&str, EntryKind)] = &[
    ("home/.cache/huggingface/stored_tokens", File),
    ("home/.cache/huggingface/token", File),
];
const COMMON_MODELS: &[(&str, EntryKind)] = &[
    (
        "home/.cache/huggingface/hub/models--TencentARC--Pixal3D",
        Directory,
    ),
    (
        "home/.cache/huggingface/hub/models--facebook--dinov3-vitl16-pretrain-lvd1689m",
        Directory,
    ),
    (
        "home/.cache/huggingface/hub/.locks/models--TencentARC--Pixal3D",
        Directory,
    ),
    (
        "home/.cache/huggingface/hub/.locks/models--facebook--dinov3-vitl16-pretrain-lvd1689m",
        Directory,
    ),
    ("home/.cache/huggingface/xet", Directory),
];
const MLX_MODELS: &[(&str, EntryKind)] = &[
    ("weights/download-manifest.json", File),
    ("weights/naf_release.pth", File),
    ("weights/naf_release.download", File),
    ("weights/naf_release.safetensors", File),
    ("weights/.naf_release.safetensors.tmp", File),
];
const CUDA_MODELS: &[(&str, EntryKind)] = &[
    (
        "home/.cache/huggingface/hub/pixal3d-cuda/naf_release.pth",
        File,
    ),
    (
        "home/.cache/huggingface/hub/pixal3d-cuda/naf_release.download",
        File,
    ),
];

struct LocalEntry {
    path: PathBuf,
    kind: EntryKind,
    size: u64,
}

fn non_managed_path() -> CoreError {
    CoreError::OperationUnavailable(
        "Studio can only remove files from the default managed runtime directories, without symlinks in the target path.".into(),
    )
}

fn inspect_entry<'a>(
    root: &Path,
    parts: impl Iterator<Item = &'a str>,
    kind: EntryKind,
) -> CoreResult<Option<LocalEntry>> {
    let mut path = root.to_path_buf();
    let mut parts = parts.peekable();
    while let Some(part) = parts.next() {
        path.push(part);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let last = parts.peek().is_none();
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const REPARSE_POINT: u32 = 0x400;
            if metadata.file_attributes() & REPARSE_POINT != 0 {
                return Err(non_managed_path());
            }
        }
        let valid = if last && matches!(kind, File) {
            metadata.file_type().is_file()
        } else {
            metadata.file_type().is_dir()
        };
        if !valid {
            return Err(non_managed_path());
        }
        if last {
            return Ok(Some(LocalEntry {
                path,
                kind,
                size: metadata.len(),
            }));
        }
    }
    Ok(None)
}

fn remove_entries(entries: Vec<LocalEntry>) -> CoreResult<()> {
    for entry in entries {
        match entry.kind {
            Directory => fs::remove_dir_all(entry.path)?,
            File => fs::remove_file(entry.path)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn model_removal_rejects_linked_parents_before_removing_other_targets() {
        use std::os::unix::fs::symlink;

        let root =
            std::env::temp_dir().join(format!("meshharbor-linked-models-{}", uuid::Uuid::new_v4()));
        let runtime = root.join("runtimes/pixal3d-mlx");
        let repo = runtime.join("home/.cache/huggingface/hub/models--TencentARC--Pixal3D");
        let external = root.join("not-managed");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("naf_release.pth"), b"keep").unwrap();
        fs::write(repo.join("cached"), b"keep on rejection").unwrap();
        symlink(&external, runtime.join("weights")).unwrap();
        assert!(Pixal3dProfile::Mlx.remove_models(&root).is_err());
        assert!(external.join("naf_release.pth").is_file());
        assert!(repo.join("cached").is_file());
        assert!(!Pixal3dProfile::Mlx.local_files(&root).model_files_managed);

        fs::remove_file(runtime.join("weights")).unwrap();
        symlink(external.join("naf_release.pth"), repo.join("snapshot-file")).unwrap();
        Pixal3dProfile::Mlx.remove_models(&root).unwrap();
        assert!(!repo.exists());
        assert!(external.join("naf_release.pth").is_file());
        fs::remove_dir_all(root).unwrap();
    }
}
