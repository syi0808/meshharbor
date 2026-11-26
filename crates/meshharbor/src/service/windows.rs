use std::{
    env, fs, io,
    net::SocketAddr,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
    ptr,
    sync::Mutex,
    time::Duration,
};

use tracing_subscriber::EnvFilter;
use uuid::Uuid;
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError, SetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Console::FreeConsole,
        Threading::{CREATE_NO_WINDOW, CreateEventW, WaitForSingleObject},
    },
};

use super::{ServiceCommand, validate_listen};

pub(super) fn run(command: ServiceCommand, root: &Path) -> io::Result<()> {
    let operation = match command {
        ServiceCommand::Install { .. } => "install",
        ServiceCommand::Start => "start",
        ServiceCommand::Stop => "stop",
        ServiceCommand::Status => "status",
        ServiceCommand::Uninstall => "uninstall",
    };
    let mut task = task_command(operation)?;
    if let ServiceCommand::Install { listen } = command {
        validate_listen(listen)?;
        let binary = env::current_exe()?;
        let root = std::path::absolute(root)?;
        task.env("MESHHARBOR_TASK_BINARY", task_path(&binary)?).env(
            "MESHHARBOR_TASK_ARGUMENTS",
            format!("--data-dir {} serve --listen {listen}", quoted_path(&root)?),
        );
    }
    run_task(task, operation)
}

pub(super) fn remove_for_uninstall(root: &Path, binary: &Path) -> io::Result<()> {
    let operation = "remove-installation";
    let mut task = task_command(operation)?;
    task.env("MESHHARBOR_TASK_BINARY", binary).env(
        "MESHHARBOR_TASK_ARGUMENTS",
        format!("--data-dir {} serve --listen ", quoted_path(root)?),
    );
    run_task(task, operation)
}

pub(crate) fn powershell(script: &str) -> io::Result<Command> {
    let powershell =
        absolute_env_path("SystemRoot")?.join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = Command::new(powershell);
    command.creation_flags(CREATE_NO_WINDOW).args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]);
    Ok(command)
}

fn task_command(operation: &str) -> io::Result<Command> {
    let mut task = powershell(include_str!("windows_task.ps1"))?;
    task.env("MESHHARBOR_TASK_OPERATION", operation);
    Ok(task)
}

fn run_task(mut task: Command, operation: &str) -> io::Result<()> {
    let status = task.status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "Windows Task Scheduler could not complete 'service {operation}' ({status})."
        )));
    }
    Ok(())
}

pub(super) fn log_path() -> io::Result<PathBuf> {
    Ok(absolute_env_path("LOCALAPPDATA")?.join("MeshHarbor/service/serve.log"))
}

fn absolute_env_path(name: &str) -> io::Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other(format!("{name} must name an absolute directory.")))
}

fn task_path(path: &Path) -> io::Result<&str> {
    path.to_str()
        .filter(|value| !value.contains(|character: char| {
            character.is_control() || matches!(character, '%' | '"')
        }))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Windows service paths must be Unicode without control characters, percent signs, or quotes: {path:?}"),
            )
        })
}

fn quoted_path(path: &Path) -> io::Result<String> {
    let path = task_path(path)?;
    let trailing_backslashes = path.len() - path.trim_end_matches('\\').len();
    Ok(format!(r#""{path}{}""#, "\\".repeat(trailing_backslashes)))
}

struct StopEvent(OwnedHandle);

impl StopEvent {
    fn create(id: Uuid) -> io::Result<Self> {
        let name: Vec<u16> = format!(r"Global\MeshHarbor.Stop.{id}")
            .encode_utf16()
            .chain([0])
            .collect();
        let (handle, error) = unsafe {
            SetLastError(0);
            let handle = CreateEventW(ptr::null(), 1, 0, name.as_ptr());
            (handle, GetLastError())
        };
        if handle.is_null() {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        if error == ERROR_ALREADY_EXISTS {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "MeshHarbor task is already running.",
            ));
        }
        Ok(Self(handle))
    }

    async fn wait(self) {
        loop {
            match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
                WAIT_OBJECT_0 => return,
                WAIT_TIMEOUT => tokio::time::sleep(Duration::from_millis(200)).await,
                _ => {
                    tracing::error!(error = %io::Error::last_os_error(), "Could not wait for Windows task stop event");
                    return;
                }
            }
        }
    }
}

pub(crate) async fn serve(
    root: &Path,
    listen: SocketAddr,
    id: Uuid,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_listen(listen)?;
    let log_path = log_path()?;
    fs::create_dir_all(log_path.parent().expect("service log has a parent"))?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("meshharbor=info".parse()?))
        .with_ansi(false)
        .with_writer(Mutex::new(log))
        .init();

    let result = async {
        let stop = StopEvent::create(id)?;
        if unsafe { FreeConsole() } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        crate::serve(root, listen, stop.wait(), |message| {
            tracing::info!("{message}")
        })
        .await
    }
    .await;
    if let Err(error) = &result {
        tracing::error!(%error, "MeshHarbor scheduled server exited");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Threading::SetEvent;

    #[test]
    fn task_paths_are_quoted_without_scheduler_expansion() {
        assert_eq!(
            quoted_path(Path::new(r"C:\mesh harbor")).unwrap(),
            r#""C:\mesh harbor""#
        );
        assert_eq!(quoted_path(Path::new(r"C:\")).unwrap(), r#""C:\\""#);
        assert!(quoted_path(Path::new(r"C:\%USERPROFILE%")).is_err());
    }

    #[tokio::test]
    async fn stop_event_is_unique_and_wakes_the_server() {
        let id = Uuid::new_v4();
        let stop = StopEvent::create(id).unwrap();
        assert!(
            matches!(StopEvent::create(id), Err(error) if error.kind() == io::ErrorKind::AlreadyExists)
        );
        assert_ne!(unsafe { SetEvent(stop.0.as_raw_handle()) }, 0);
        tokio::time::timeout(Duration::from_secs(2), stop.wait())
            .await
            .unwrap();
        assert!(StopEvent::create(id).is_ok());
    }
}
