#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Installation {
    profile: tempfile::TempDir,
    data: PathBuf,
    binary: PathBuf,
}

impl Installation {
    fn new() -> Self {
        let profile = tempfile::tempdir().unwrap();
        let data = profile.path().join("app data");
        let binary = profile.path().join("meshharbor");
        fs::copy(env!("CARGO_BIN_EXE_meshharbor"), &binary).unwrap();
        Self {
            profile,
            data,
            binary,
        }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.command(&self.data).args(arguments).output().unwrap()
    }

    fn command(&self, data: &Path) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .env("HOME", self.profile.path())
            .env("XDG_CONFIG_HOME", self.profile.path().join("config"))
            .arg("--data-dir")
            .arg(data);
        command
    }
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn preview_is_read_only_and_confirmation_removes_only_the_installation() {
    let installation = Installation::new();
    let preview = success(installation.run(&["uninstall"]));
    assert!(preview.contains("Preview only"));
    assert!(preview.contains("app data"));
    assert!(!installation.data.exists());
    assert!(installation.binary.is_file());

    success(installation.run(&["local", "assets"]));
    let runtimes = installation.data.join("runtimes");
    fs::create_dir(&runtimes).unwrap();
    let external = installation.profile.path().join("shared weights");
    fs::create_dir(&external).unwrap();
    let model = external.join("weights");
    fs::write(&model, "keep").unwrap();
    std::os::unix::fs::symlink(&external, runtimes.join("shared")).unwrap();
    fs::write(runtimes.join("managed"), "remove").unwrap();
    let logs = installation.profile.path().join("Library/Logs/MeshHarbor");
    if cfg!(target_os = "macos") {
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join("serve.log"), "remove").unwrap();
        fs::write(logs.join("unrelated"), "keep").unwrap();
    }

    success(installation.run(&["uninstall"]));
    assert!(runtimes.join("managed").is_file());
    if cfg!(target_os = "macos") {
        assert!(logs.join("serve.log").is_file());
    }
    success(installation.run(&["uninstall", "--confirm"]));

    assert!(!installation.binary.exists());
    assert!(!installation.data.exists());
    assert_eq!(fs::read_to_string(model).unwrap(), "keep");
    if cfg!(target_os = "macos") {
        assert!(!logs.join("serve.log").exists());
        assert!(logs.join("unrelated").is_file());
    }
}

#[test]
fn rejects_unexpected_entries_and_linked_roots_then_can_keep_the_binary() {
    let installation = Installation::new();
    success(installation.run(&["local", "assets"]));
    let unrelated = installation.data.join("personal.txt");
    fs::write(&unrelated, "do not remove").unwrap();
    let result = installation.run(&["uninstall", "--confirm"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unexpected entry"));
    assert!(installation.data.join("meshharbor.sqlite3").is_file());
    assert!(installation.binary.is_file());
    assert!(unrelated.is_file());
    fs::remove_file(unrelated).unwrap();

    let link = installation.profile.path().join("linked");
    std::os::unix::fs::symlink(&installation.data, &link).unwrap();
    let result = installation
        .command(&link.join("."))
        .args(["uninstall", "--confirm"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(installation.data.join("meshharbor.sqlite3").is_file());

    success(installation.run(&["uninstall", "--confirm", "--keep-binary"]));
    assert!(!installation.data.exists());
    assert!(installation.binary.is_file());
    success(installation.run(&["uninstall", "--confirm", "--keep-binary"]));
}

#[cfg(target_os = "linux")]
#[test]
fn only_removes_the_matching_user_service() {
    use std::os::unix::fs::PermissionsExt;

    let installation = Installation::new();
    let bin_dir = installation.profile.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let systemctl = bin_dir.join("systemctl");
    fs::write(
        &systemctl,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$SYSTEMCTL_CALLS\"\n",
    )
    .unwrap();
    fs::set_permissions(systemctl, fs::Permissions::from_mode(0o700)).unwrap();
    let calls = installation.profile.path().join("systemctl-calls");
    let run = |data: &Path, arguments: &[&str]| {
        installation
            .command(data)
            .args(arguments)
            .env("PATH", &bin_dir)
            .env("SYSTEMCTL_CALLS", &calls)
            .output()
            .unwrap()
    };
    let unit = installation
        .profile
        .path()
        .join("config/systemd/user/meshharbor.service");

    success(run(
        &installation.data,
        &["service", "install", "--listen", "127.0.0.1:7441"],
    ));
    assert!(unit.is_file());
    let other = installation.profile.path().join("different data");
    let result = run(&other, &["uninstall", "--confirm"]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("different executable or data directory")
    );
    assert!(unit.is_file());
    assert!(installation.binary.is_file());

    success(run(&installation.data, &["uninstall", "--confirm"]));
    assert!(!unit.exists());
    assert!(!installation.binary.exists());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(calls.ends_with(
        "--user --no-pager disable --now meshharbor.service\n--user --no-pager daemon-reload\n"
    ));
}
