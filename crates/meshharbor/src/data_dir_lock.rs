use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

/// Coordinates service, recovery, generation, and runtime-install processes using one data directory.
///
/// REST/MCP services share the service lock; recovery takes it exclusively. Generation obtains a
/// shared runtime lock, while runtime setup, download, login, and conversion obtain that same lock
/// exclusively. The independent runtime lock file is advisory; the OS releases it if a process exits
/// or crashes.
#[derive(Debug)]
pub struct DataDirLock {
    _file: File,
}

#[derive(Clone, Copy)]
enum LockMode {
    Shared,
    Exclusive,
}

impl DataDirLock {
    pub fn try_acquire_for_service(root: &Path) -> io::Result<Self> {
        Self::try_acquire(root, ".meshharbor.lock", LockMode::Shared)
    }

    pub fn try_acquire_for_recovery(root: &Path) -> io::Result<Self> {
        Self::try_acquire(root, ".meshharbor.lock", LockMode::Exclusive)
    }

    /// Acquire exclusive access to app-managed runtime state and shared Python caches.
    pub fn try_acquire_for_runtime(root: &Path) -> io::Result<Self> {
        Self::try_acquire(root, ".meshharbor.runtime.lock", LockMode::Exclusive)
    }

    /// Acquire read access for model inference, excluding installs and cache mutation.
    pub fn try_acquire_for_generation(root: &Path) -> io::Result<Self> {
        Self::try_acquire(root, ".meshharbor.runtime.lock", LockMode::Shared)
    }

    fn try_acquire(root: &Path, lock_file_name: &str, mode: LockMode) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(lock_file_name))?;
        lock_file(&file, mode)?;
        Ok(Self { _file: file })
    }
}

#[cfg(unix)]
fn lock_file(file: &File, mode: LockMode) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    const LOCK_SH: std::ffi::c_int = 1;
    const LOCK_EX: std::ffi::c_int = 2;
    const LOCK_NB: std::ffi::c_int = 4;

    unsafe extern "C" {
        fn flock(fd: std::os::fd::RawFd, operation: std::ffi::c_int) -> std::ffi::c_int;
    }

    let operation = match mode {
        LockMode::Shared => LOCK_SH,
        LockMode::Exclusive => LOCK_EX,
    };
    let result = unsafe { flock(file.as_raw_fd(), operation | LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        map_lock_error(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn lock_file(file: &File, mode: LockMode) -> io::Result<()> {
    use std::{ffi::c_void, os::windows::io::AsRawHandle};

    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x1;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x2;
    const ERROR_LOCK_VIOLATION: i32 = 33;

    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut c_void,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LockFileEx(
            file: *mut c_void,
            flags: u32,
            reserved: u32,
            bytes_low: u32,
            bytes_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }

    let mut overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: std::ptr::null_mut(),
    };
    let mode_flag = match mode {
        LockMode::Shared => 0,
        LockMode::Exclusive => LOCKFILE_EXCLUSIVE_LOCK,
    };
    let result = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_FAIL_IMMEDIATELY | mode_flag,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if result != 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
            Err(in_use_error())
        } else {
            Err(error)
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn lock_file(_file: &File, _mode: LockMode) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "MeshHarbor data-directory locking is not supported on this platform.",
    ))
}

#[cfg(unix)]
fn map_lock_error(error: io::Error) -> io::Result<()> {
    if error.kind() == io::ErrorKind::WouldBlock {
        Err(in_use_error())
    } else {
        Err(error)
    }
}

#[cfg(any(unix, windows))]
fn in_use_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "another MeshHarbor process holds an incompatible lock for this data directory",
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::DataDirLock;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "meshharbor-data-dir-lock-{}-{number}",
            std::process::id()
        ))
    }

    #[test]
    fn services_share_lock_but_recovery_is_exclusive() {
        let root = temp_dir();
        let first_service = DataDirLock::try_acquire_for_service(&root).unwrap();
        let second_service = DataDirLock::try_acquire_for_service(&root).unwrap();

        let recovery_while_serving = DataDirLock::try_acquire_for_recovery(&root).unwrap_err();
        assert_eq!(
            recovery_while_serving.kind(),
            std::io::ErrorKind::WouldBlock
        );

        drop(first_service);
        drop(second_service);
        let recovery = DataDirLock::try_acquire_for_recovery(&root).unwrap();
        let service_while_recovering = DataDirLock::try_acquire_for_service(&root).unwrap_err();
        assert_eq!(
            service_while_recovering.kind(),
            std::io::ErrorKind::WouldBlock
        );

        drop(recovery);
        let released = DataDirLock::try_acquire_for_service(&root).unwrap();
        drop(released);
        fs::remove_dir_all(root).unwrap();
    }


    #[test]
    fn runtime_install_lock_excludes_generation_but_allows_concurrent_readers() {
        let root = temp_dir();
        let other_root = root.with_extension("other");

        let service = DataDirLock::try_acquire_for_service(&root).unwrap();
        let installer = DataDirLock::try_acquire_for_runtime(&root).unwrap();
        assert_eq!(
            DataDirLock::try_acquire_for_generation(&root)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(
            DataDirLock::try_acquire_for_runtime(&root)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        // Runtime actions on independent MeshHarbor data roots do not block one another.
        let independent_reader = DataDirLock::try_acquire_for_generation(&other_root).unwrap();
        drop(independent_reader);
        drop(installer);

        let first_reader = DataDirLock::try_acquire_for_generation(&root).unwrap();
        let second_reader = DataDirLock::try_acquire_for_generation(&root).unwrap();
        assert_eq!(
            DataDirLock::try_acquire_for_runtime(&root)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(first_reader);
        drop(second_reader);
        let installer_after_readers = DataDirLock::try_acquire_for_runtime(&root).unwrap();
        drop(installer_after_readers);
        drop(service);

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(other_root).unwrap();
    }

}
