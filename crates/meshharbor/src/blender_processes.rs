use std::{io, path::Path};

fn is_blender(executable: &str) -> bool {
    Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            matches!(name.as_str(), "blender" | "blender.exe" | "blender-bin")
                || (name.starts_with("blender-") && name.ends_with(".appimage"))
        })
}

#[cfg(unix)]
pub(super) fn blender_running() -> io::Result<bool> {
    let ps = ["/bin/ps", "/usr/bin/ps"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "ps is not available"))?;
    let output = std::process::Command::new(ps)
        .args(["-A", "-ww", "-o", "stat=,comm="])
        .output()?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(io::Error::other("could not enumerate processes"));
    }
    Ok(ps_lists_blender(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(unix)]
fn ps_lists_blender(output: &str) -> bool {
    output.lines().any(|line| {
        line.trim_start()
            .split_once(char::is_whitespace)
            .is_some_and(|(state, executable)| {
                !state.starts_with('Z') && is_blender(executable.trim())
            })
    })
}

#[cfg(windows)]
pub(super) fn blender_running() -> io::Result<bool> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let result = if unsafe { Process32FirstW(snapshot, &mut entry) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        loop {
            let length = entry
                .szExeFile
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(entry.szExeFile.len());
            if is_blender(&String::from_utf16_lossy(&entry.szExeFile[..length])) {
                break Ok(true);
            }
            if unsafe { Process32NextW(snapshot, &mut entry) } == 0 {
                let error = io::Error::last_os_error();
                break if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                    Ok(false)
                } else {
                    Err(error)
                };
            }
        }
    };
    unsafe { CloseHandle(snapshot) };
    result
}

#[cfg(not(any(unix, windows)))]
pub(super) fn blender_running() -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process enumeration is not available",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_blender_executables_not_command_arguments_or_helpers() {
        for executable in [
            "Blender",
            "/Applications/Blender Nightly.app/Contents/MacOS/Blender",
            "blender.exe",
            "blender-bin",
            "Blender-4.5.AppImage",
        ] {
            assert!(is_blender(executable), "{executable}");
        }
        for executable in [
            "meshharbor",
            "blender-thumbnailer",
            "python.exe",
            "python --blender",
            "blender.exe.backup",
        ] {
            assert!(!is_blender(executable), "{executable}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn process_listing_recognizes_spaced_paths_and_ignores_zombies() {
        assert!(ps_lists_blender(
            " Ss    /Applications/Blender New.app/Contents/MacOS/Blender\n"
        ));
        assert!(!ps_lists_blender(" Z    Blender\nS   /bin/ps\n"));
    }
}
