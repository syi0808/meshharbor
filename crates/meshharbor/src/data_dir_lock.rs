use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

/// Keeps one service or recovery command from racing another process using the same data directory.
///
/// The lock file itself is intentionally persistent. The operating system releases the advisory
/// lock when the owning process exits, including after a crash.
#[derive(Debug)]
pub struct DataDirLock {
    _file: File,
}

impl DataDirLock {
    pub fn try_acquire(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".meshharbor.lock"))?;
        lock_file(&file)?;
        Ok(Self { _file: file })
    }
}

#[cfg(unix)]
fn lock_file(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    const LOCK_EX: std::ffi::c_int = 2;
    const LOCK_NB: std::ffi::c_int = 4;

    unsafe extern "C" {
        fn flock(fd: std::os::fd::RawFd, operation: std::ffi::c_int) -> std::ffi::c_int;
    }

    let result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        map_lock_error(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn lock_file(file: &File) -> io::Result<()> {
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
    let result = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_FAIL_IMMEDIATELY | LOCKFILE_EXCLUSIVE_LOCK,
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
fn lock_file(_file: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "MeshHarbor data-directory locking is not supported on this platform.",
    ))
}

fn map_lock_error(error: io::Error) -> io::Result<()> {
    if error.kind() == io::ErrorKind::WouldBlock {
        Err(in_use_error())
    } else {
        Err(error)
    }
}

fn in_use_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "another MeshHarbor service or recovery command already holds this data directory",
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
    fn lock_is_exclusive_and_released_when_guard_drops() {
        let root = temp_dir();
        let first = DataDirLock::try_acquire(&root).unwrap();

        let second = DataDirLock::try_acquire(&root).unwrap_err();
        assert_eq!(second.kind(), std::io::ErrorKind::WouldBlock);
        assert!(
            second
                .to_string()
                .contains("already holds this data directory")
        );

        drop(first);
        let released = DataDirLock::try_acquire(&root).unwrap();
        drop(released);
        fs::remove_dir_all(root).unwrap();
    }
}
