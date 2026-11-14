use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

/// Coordinates service and recovery processes that use the same data directory.
///
/// Services take shared locks so REST and MCP can run together. Recovery takes an exclusive lock,
/// so it cannot mark jobs failed while a service is active. The persistent lock file is harmless:
/// the operating system releases its advisory lock when the owning process exits or crashes.
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
        Self::try_acquire(root, LockMode::Shared)
    }

    pub fn try_acquire_for_recovery(root: &Path) -> io::Result<Self> {
        Self::try_acquire(root, LockMode::Exclusive)
    }

    fn try_acquire(root: &Path, mode: LockMode) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".meshharbor.lock"))?;
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
        "a MeshHarbor service or recovery command is using this data directory",
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
}
