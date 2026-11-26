use std::{
    env, fs, io,
    path::{Component, Path, PathBuf},
};

use crate::{
    data_dir_lock::{DataDirLock, RUNTIME_LOCK_FILE, SERVICE_LOCK_FILE},
    service,
};

const DATABASE: &str = "meshharbor.sqlite3";

pub(crate) fn run(root: &Path, confirm: bool, keep_binary: bool) -> io::Result<()> {
    let service_root = std::path::absolute(root)?;
    let service_binary = env::current_exe()?;
    let binary = fs::canonicalize(&service_binary)?;
    let log = service::log_file()?;
    let log_data_root = log
        .as_deref()
        .filter(|_| cfg!(windows))
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(|path| path.canonicalize().ok());
    let data = checked_data_root(&service_root, &binary, log_data_root.as_deref())?;

    println!(
        "Data directory: {}{}",
        data.as_ref().unwrap_or(&service_root).display(),
        if data.is_none() { " (not present)" } else { "" }
    );
    println!("Background service: remove the matching current-user registration, if installed");
    if let Some(log) = &log {
        println!("Service log: {}", log.display());
    }
    println!(
        "Executable: {}{}",
        binary.display(),
        if keep_binary { " (keep)" } else { "" }
    );
    if !confirm {
        println!("Preview only. Pass --confirm to permanently delete the listed data.");
        return Ok(());
    }

    service::remove_for_uninstall(&service_root, &service_binary)?;
    let current_data = checked_data_root(&service_root, &binary, log_data_root.as_deref())?;
    if data != current_data {
        return Err(io::Error::other(
            "The data directory changed; rerun uninstall to review it.",
        ));
    }
    if let Some(data) = current_data {
        remove_data(&data, log_data_root.as_ref() == Some(&data))?;
        println!("Removed data: {}", data.display());
    }
    if let Some(log) = log {
        remove_log(&log)?;
    }
    if !keep_binary {
        #[cfg(windows)]
        let binary = service_binary;
        remove_executable(&binary)?;
    }
    Ok(())
}

fn checked_data_root(
    root: &Path,
    binary: &Path,
    log_data_root: Option<&Path>,
) -> io::Result<Option<PathBuf>> {
    let root: PathBuf = root.components().collect();
    if root.file_name().is_none() || root.components().any(|part| part == Component::ParentDir) {
        return Err(unsafe_directory(&root));
    }
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.is_dir() && !is_link(&metadata) => {}
        Ok(_) => return Err(unsafe_directory(&root)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let root = fs::canonicalize(root)?;
    let cwd = env::current_dir()?.canonicalize()?;
    let protected = ["HOME", "USERPROFILE", "XDG_DATA_HOME", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(env::var_os)
        .map(PathBuf::from)
        .chain([env::temp_dir()])
        .filter_map(|path| path.canonicalize().ok());
    if binary.starts_with(&root)
        || cwd.starts_with(&root)
        || protected.into_iter().any(|path| path.starts_with(&root))
    {
        return Err(unsafe_directory(&root));
    }
    validate_entries(&root, log_data_root == Some(&root))?;
    Ok(Some(root))
}

fn validate_entries(root: &Path, allow_service_log: bool) -> io::Result<Vec<(PathBuf, bool)>> {
    let mut has_database = false;
    let mut entries = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        let known = !is_link(&metadata)
            && match entry.file_name().to_str() {
                Some(SERVICE_LOCK_FILE | RUNTIME_LOCK_FILE) => metadata.is_file(),
                Some(DATABASE) => {
                    has_database = metadata.is_file();
                    has_database
                }
                Some(
                    "meshharbor.sqlite3-shm"
                    | "meshharbor.sqlite3-wal"
                    | "meshharbor.sqlite3-journal",
                ) => {
                    entries.push((entry.path(), false));
                    metadata.is_file()
                }
                Some(
                    "artifacts"
                    | "staging"
                    | "backends"
                    | "model-environment"
                    | "runtimes"
                    | "tools"
                    | "exports"
                    | ".meshharbor-job-owners",
                ) => {
                    entries.push((entry.path(), true));
                    metadata.is_dir()
                }
                Some("service") if allow_service_log && metadata.is_dir() => {
                    entries.push((entry.path(), true));
                    service_log_directory(&entry.path())?
                }
                _ => false,
            };
        if !known {
            return Err(io::Error::other(format!(
                "Refusing to delete a data directory with an unexpected entry: {}. Move it out and try again.",
                entry.path().display()
            )));
        }
    }
    if !has_database
        && entries
            .iter()
            .any(|(path, _)| path.file_name().is_some_and(|name| name != "service"))
    {
        return Err(io::Error::other(format!(
            "No MeshHarbor database found in {}. Refusing to delete an unrecognized data directory.",
            root.display()
        )));
    }
    Ok(entries)
}

fn service_log_directory(directory: &Path) -> io::Result<bool> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if entry.file_name() != "serve.log" || !metadata.is_file() || is_link(&metadata) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

fn unsafe_directory(root: &Path) -> io::Error {
    io::Error::other(format!(
        "Refusing to delete {}. Select a dedicated MeshHarbor data directory and run from outside it.",
        root.display()
    ))
}

fn remove_data(root: &Path, allow_service_log: bool) -> io::Result<()> {
    let lock_error = |error: io::Error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            io::Error::other(
                "MeshHarbor is still using this data directory. Close Studio's server, MCP clients, and other MeshHarbor commands, then retry.",
            )
        } else {
            error
        }
    };
    let store = DataDirLock::try_acquire_for_recovery(root).map_err(lock_error)?;
    let runtime = DataDirLock::try_acquire_for_runtime(root).map_err(lock_error)?;
    for (path, is_directory) in validate_entries(root, allow_service_log)? {
        if is_directory {
            fs::remove_dir_all(path)?;
        } else {
            remove_file_if_present(&path)?;
        }
    }
    remove_file_if_present(&root.join(DATABASE))?;
    remove_file_if_present(&root.join(RUNTIME_LOCK_FILE))?;
    remove_file_if_present(&root.join(SERVICE_LOCK_FILE))?;
    drop((store, runtime));
    fs::remove_dir(root)
}

fn remove_log(log: &Path) -> io::Result<()> {
    let directory = log.parent().expect("service log has a parent");
    #[cfg(windows)]
    let parents = [
        directory,
        directory
            .parent()
            .expect("service log has a data directory"),
    ];
    #[cfg(not(windows))]
    let parents = [directory];
    for parent in parents {
        match fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.is_dir() && !is_link(&metadata) => {}
            Ok(_) => return Err(unsafe_directory(parent)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    remove_file_if_present(log)?;
    match fs::remove_dir(directory) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::NotFound
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(not(windows))]
fn remove_executable(binary: &Path) -> io::Result<()> {
    fs::remove_file(binary)?;
    println!("Removed executable: {}", binary.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletion_waits_for_store_and_runtime_owners_to_exit() {
        let _test_guard = crate::DATA_DIR_LOCK_TEST_MUTEX.lock().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("data");
        drop(meshharbor_core::OperationCore::open(&root).unwrap());

        let service = DataDirLock::try_acquire_for_service(&root).unwrap();
        assert!(
            remove_data(&root, false)
                .unwrap_err()
                .to_string()
                .contains("still using")
        );
        assert!(root.join(DATABASE).is_file());
        drop(service);

        let runtime = DataDirLock::try_acquire_for_generation(&root).unwrap();
        assert!(
            remove_data(&root, false)
                .unwrap_err()
                .to_string()
                .contains("still using")
        );
        assert!(root.join(DATABASE).is_file());
        drop(runtime);

        remove_data(&root, false).unwrap();
        assert!(!root.exists());
    }
}

#[cfg(windows)]
fn remove_executable(binary: &Path) -> io::Result<()> {
    use std::process::Stdio;

    let mut cleanup = service::windows::powershell(include_str!("uninstall_windows.ps1"))?;
    let pending =
        binary.with_file_name(format!("meshharbor-uninstall-{}.exe", uuid::Uuid::new_v4()));
    fs::rename(binary, &pending)?;
    if let Err(error) = cleanup
        .env("MESHHARBOR_UNINSTALL_BINARY", &pending)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        fs::rename(&pending, binary).map_err(|restore| {
            io::Error::other(format!(
                "Could not start executable cleanup ({error}) or restore its original name ({restore}). Remove {} manually.",
                pending.display()
            ))
        })?;
        return Err(error);
    }
    println!(
        "Executable cleanup runs after exit. If it remains, remove {} manually.",
        pending.display()
    );
    Ok(())
}
