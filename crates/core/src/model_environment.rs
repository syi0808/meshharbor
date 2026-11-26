use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    CoreError, CoreResult,
    huggingface_browser::PACKAGE,
    job_execution::JobExecution,
    model_storage::{managed_directory, managed_file_size},
    uv_bootstrap::{ensure_uv_tracked, is_executable, local_tool_dirs, uv_available},
};

const DIRECTORY: &str = "model-environment";
const MANIFEST: &str = "model-environment/tools-v1.json";
const PYTHONS: [&str; 2] = ["3.10", "3.12"];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelEnvironmentStatus {
    pub ready: bool,
    pub python_versions: Vec<String>,
    #[serde(default)]
    pub uv_found: bool,
    #[serde(default)]
    pub python_versions_found: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct Tools {
    uv: PathBuf,
    python310: PathBuf,
    python312: PathBuf,
}

pub(crate) struct ModelEnvironment {
    directory: PathBuf,
    tools: Tools,
}

impl ModelEnvironment {
    pub(crate) fn status(root: &Path) -> ModelEnvironmentStatus {
        let ready = Self::load(root).is_some();
        let tool_dirs = local_tool_dirs();
        let installs = [
            Some(root.join(DIRECTORY).join("python")),
            env::var_os("UV_PYTHON_INSTALL_DIR").map(PathBuf::from),
            env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
                })
                .map(|data| data.join("uv/python")),
        ];
        ModelEnvironmentStatus {
            ready,
            python_versions: PYTHONS.map(str::to_owned).into(),
            uv_found: ready || uv_available(root),
            python_versions_found: PYTHONS
                .into_iter()
                .filter(|version| {
                    ready
                        || python_on_search_path(&tool_dirs, version)
                        || installs
                            .iter()
                            .flatten()
                            .any(|directory| uv_python_is_present(directory, version))
                })
                .map(str::to_owned)
                .collect(),
        }
    }

    pub(crate) fn load(root: &Path) -> Option<Self> {
        if !managed_file_size(root, MANIFEST)
            .ok()
            .flatten()
            .is_some_and(|size| size < 8192)
        {
            return None;
        }
        let directory = fs::canonicalize(root.join(DIRECTORY)).ok()?;
        let tools: Tools = serde_json::from_slice(&fs::read(root.join(MANIFEST)).ok()?).ok()?;
        let environment = Self { directory, tools };
        (environment.tools.uv.is_absolute()
            && environment.uv().is_file()
            && [environment.python("3.10"), environment.python("3.12")]
                .into_iter()
                .all(|python| python.is_absolute() && python.is_file())
            && environment.hf_python().is_file()
            && environment.hf_cli().is_file())
        .then_some(environment)
    }

    pub(crate) fn require(root: &Path) -> CoreResult<Self> {
        Self::load(root).ok_or_else(|| CoreError::OperationUnavailable(
            "Set up Python and uv in Studio → Settings → Environment first (CLI: meshharbor runtime environment-setup).".into(),
        ))
    }

    pub(crate) fn uv(&self) -> &Path {
        &self.tools.uv
    }

    pub(crate) fn python(&self, version: &str) -> &Path {
        match version {
            "3.10" => &self.tools.python310,
            "3.12" => &self.tools.python312,
            _ => unreachable!("only configured Python versions can be used"),
        }
    }

    pub(crate) fn hf_python(&self) -> PathBuf {
        self.directory.join("huggingface").join(python_executable())
    }

    pub(crate) fn hf_cli(&self) -> PathBuf {
        self.directory.join("huggingface").join(if cfg!(windows) {
            "Scripts/hf.exe"
        } else {
            "bin/hf"
        })
    }

    pub(crate) fn prepare(
        root: &Path,
        execution: JobExecution<'_>,
    ) -> CoreResult<ModelEnvironmentStatus> {
        execution.stage("prepare_runtime")?;
        let root = fs::canonicalize(root)?;
        let directory = root.join(DIRECTORY);
        for relative in [
            DIRECTORY,
            "model-environment/python",
            "model-environment/home",
            "model-environment/uv-cache",
            "model-environment/huggingface",
        ] {
            managed_directory(&root, relative)?;
        }
        fs::create_dir_all(directory.join("home"))?;
        let uv = std::path::absolute(ensure_uv_tracked(&root, execution)?)?;
        Self::prepare_with_uv(&root, directory, uv, execution)
    }

    fn prepare_with_uv(
        root: &Path,
        directory: PathBuf,
        uv: PathBuf,
        execution: JobExecution<'_>,
    ) -> CoreResult<ModelEnvironmentStatus> {
        let path = env::join_paths(local_tool_dirs())
            .unwrap_or_else(|_| env::var_os("PATH").unwrap_or_default());
        let command = |program: &Path| {
            let mut command = Command::new(program);
            command
                .env_clear()
                .current_dir(&directory)
                .stdin(Stdio::null())
                .env("PATH", &path)
                .env("HOME", directory.join("home"))
                .env("UV_PYTHON_INSTALL_DIR", directory.join("python"))
                .env("UV_CACHE_DIR", directory.join("uv-cache"))
                .env("UV_NO_CONFIG", "1");
            for key in [
                "SYSTEMROOT",
                "TEMP",
                "TMP",
                "TMPDIR",
                "SSL_CERT_FILE",
                "SSL_CERT_DIR",
            ] {
                if let Some(value) = env::var_os(key) {
                    command.env(key, value);
                }
            }
            command
        };
        execution.capture(command(&uv).arg("--version"), "Could not run uv")?;
        execution.stage("prepare_python")?;
        let python_dir = directory.join("python");
        let find = |version: &str, managed: bool| -> CoreResult<Option<PathBuf>> {
            let mut discovery = command(&uv);
            if !managed {
                discovery
                    .env_remove("HOME")
                    .env_remove("UV_PYTHON_INSTALL_DIR");
                for key in [
                    "HOME",
                    "USERPROFILE",
                    "APPDATA",
                    "LOCALAPPDATA",
                    "XDG_DATA_HOME",
                    "UV_PYTHON_INSTALL_DIR",
                ] {
                    if let Some(value) = env::var_os(key) {
                        discovery.env(key, value);
                    }
                }
            }
            discovery.args([
                "python",
                "find",
                "--system",
                "--offline",
                "--no-project",
                "--no-python-downloads",
            ]);
            if managed {
                discovery.arg("--managed-python");
            }
            discovery.arg(format!("cpython@{version}"));
            let path = match execution.capture(&mut discovery, "Could not locate Python") {
                Ok(output) => PathBuf::from(output),
                Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                Err(_) => return Ok(None),
            };
            if !path.is_absolute()
                || !is_executable(&path)
                || (managed && !fs::canonicalize(&path)?.starts_with(&python_dir))
            {
                return Ok(None);
            }
            match execution.capture(command(&path).arg("--version"), "Could not start Python") {
                Ok(output) if output.starts_with(&format!("Python {version}.")) => Ok(Some(path)),
                Err(CoreError::Cancelled) => Err(CoreError::Cancelled),
                _ => Ok(None),
            }
        };
        let mut interpreters = [None, None];
        for (version, interpreter) in PYTHONS.into_iter().zip(&mut interpreters) {
            *interpreter = find(version, true)?;
            if interpreter.is_none() {
                *interpreter = find(version, false)?;
                if interpreter.is_some() {
                    execution.note(&format!(
                        "Reusing an existing Python {version} interpreter."
                    ))?;
                }
            }
        }
        let missing = PYTHONS
            .into_iter()
            .zip(&interpreters)
            .filter_map(|(version, interpreter)| interpreter.is_none().then_some(version))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            execution.stage("install_python")?;
            let mut install = command(&uv);
            install
                .args(["python", "install", "--no-bin"])
                .args(missing);
            execution.visible(install, "Could not install missing Python interpreters")?;
        }
        let [python310, python312] = interpreters;
        let require_python = |version, interpreter: Option<PathBuf>| {
            if let Some(path) = interpreter {
                return Ok(path);
            }
            find(version, true)?.ok_or_else(|| {
                CoreError::OperationUnavailable(format!(
                    "uv did not return a usable Python {version} interpreter."
                ))
            })
        };
        let environment = Self {
            tools: Tools {
                python310: require_python("3.10", python310)?,
                python312: require_python("3.12", python312)?,
                uv,
            },
            directory: directory.clone(),
        };
        execution.stage("install_account_tools")?;
        let mut venv = command(environment.uv());
        venv.args([
            "venv",
            "--no-project",
            "--allow-existing",
            "--no-python-downloads",
            "--python",
        ])
        .arg(environment.python("3.12"))
        .arg(directory.join("huggingface"));
        execution.visible(
            venv,
            "Could not prepare the shared Hugging Face environment",
        )?;
        let mut pip = command(environment.uv());
        pip.args(["pip", "install", "--no-python-downloads", "--python"])
            .arg(environment.hf_python())
            .args([
                "--default-index",
                "https://pypi.org/simple",
                "--only-binary",
                ":all:",
                PACKAGE,
            ]);
        execution.visible(pip, "Could not install the shared Hugging Face tool")?;
        execution.stage("verify_environment")?;
        execution.capture(
            command(&environment.hf_cli()).arg("--help"),
            "Could not verify the shared Hugging Face tool",
        )?;
        managed_file_size(root, MANIFEST)?;
        let staged = directory.join(format!(".tools-{}.json", Uuid::new_v4()));
        execution.begin_commit()?;
        fs::write(&staged, serde_json::to_vec(&environment.tools)?)?;
        let published = fs::rename(&staged, root.join(MANIFEST));
        if published.is_err() {
            let _ = fs::remove_file(staged);
        }
        published?;
        Ok(Self::status(root))
    }
}

fn python_on_search_path(dirs: &[PathBuf], version: &str) -> bool {
    let executable = if cfg!(windows) {
        format!("python{version}.exe")
    } else {
        format!("python{version}")
    };
    dirs.iter()
        .any(|directory| is_executable(&directory.join(&executable)))
        || (cfg!(windows)
            && env::var_os("LOCALAPPDATA").is_some_and(|base| {
                let version_dir = format!("Programs/Python/Python{}", version.replace('.', ""));
                is_executable(&PathBuf::from(base).join(version_dir).join("python.exe"))
            }))
}

fn uv_python_is_present(directory: &Path, version: &str) -> bool {
    let prefix = format!("cpython-{version}.");
    fs::read_dir(directory).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry.file_name().to_string_lossy().starts_with(&prefix)
                && is_executable(&entry.path().join(if cfg!(windows) {
                    PathBuf::from("python.exe")
                } else {
                    PathBuf::from(format!("bin/python{version}"))
                }))
        })
    })
}

fn python_executable() -> &'static str {
    if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OperationCore, RuntimeJobKind};

    #[cfg(unix)]
    #[test]
    fn setup_reuses_compatible_python_and_installs_only_the_missing_version() {
        use std::os::unix::fs::PermissionsExt;

        let root = env::temp_dir().join(format!("meshharbor-python-reuse-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let directory = root.join(DIRECTORY);
        let local = root.join("local-bin");
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(directory.join("home")).unwrap();
        let put_script = |path: &Path, content| {
            fs::write(path, content).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        };
        let python312 = local.join("python3.12");
        put_script(&python312, "#!/bin/sh\necho Python 3.12.10\n");
        put_script(&local.join("python3.10"), "#!/bin/sh\necho Python 3.13.4\n");
        let uv = local.join("uv");
        put_script(
            &uv,
            r#"#!/bin/sh
set -eu
local_bin="$(dirname "$0")"
for argument; do request="$argument"; done
case "$*" in
    --version) echo "uv fixture" ;;
    "python find "*" --managed-python cpython@"*)
        version="${request#cpython@}"
        path="$UV_PYTHON_INSTALL_DIR/cpython-$version.1-fixture/bin/python$version"
        test -f "$path" && echo "$path"
        ;;
    "python find "*" cpython@"*)
        version="${request#cpython@}"
        echo "$local_bin/python$version"
        ;;
    "python install --no-bin 3.10")
        echo "$*" >> "$PWD/installs"
        path="$UV_PYTHON_INSTALL_DIR/cpython-3.10.1-fixture/bin"
        mkdir -p "$path"
        printf '#!/bin/sh\necho Python 3.10.1\n' > "$path/python3.10"
        chmod +x "$path/python3.10"
        ;;
    "venv "*)
        test "$6" = "$local_bin/python3.12"
        mkdir -p "$7/bin"
        cp "$6" "$7/bin/python"
        printf '#!/bin/sh\nexit 0\n' > "$7/bin/hf"
        chmod +x "$7/bin/hf"
        ;;
    "pip install "*) ;;
    *) exit 1 ;;
esac
"#,
        );

        for _ in 0..2 {
            let status = ModelEnvironment::prepare_with_uv(
                &root,
                directory.clone(),
                uv.clone(),
                Default::default(),
            )
            .unwrap();
            assert!(status.ready);
            let environment = ModelEnvironment::require(&root).unwrap();
            assert_eq!(environment.uv(), uv);
            assert_eq!(
                fs::canonicalize(environment.python("3.12")).unwrap(),
                python312
            );
            assert!(
                environment
                    .python("3.10")
                    .starts_with(directory.join("python"))
            );
        }
        assert_eq!(
            fs::read_to_string(directory.join("installs")).unwrap(),
            "python install --no-bin 3.10\n"
        );
        fs::remove_file(python312).unwrap();
        assert!(!ModelEnvironment::status(&root).ready);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_model_install_requires_shared_tools_and_a_saved_account() {
        let directory = env::temp_dir().join(format!("meshharbor-shared-tools-{}", Uuid::new_v4()));
        let core = OperationCore::open(&directory).unwrap();
        let root = fs::canonicalize(&directory).unwrap();
        let environment = root.join(DIRECTORY);
        let jobs = [
            RuntimeJobKind::InstallCatalogModel("pixal3d-mlx"),
            RuntimeJobKind::InstallCatalogModel("trellis2-cuda"),
            RuntimeJobKind::InstallCatalogModel("triposg-mlx"),
            RuntimeJobKind::InstallCatalogModel("trellis-text-mlx"),
            RuntimeJobKind::InstallCatalogModel("geosam2-mlx"),
            RuntimeJobKind::CudaInstallMultiview,
        ];
        assert!(!ModelEnvironment::status(&root).ready);
        for job in jobs {
            assert!(
                core.validate_runtime_job(job)
                    .unwrap_err()
                    .to_string()
                    .contains("Settings → Environment")
            );
        }
        core.validate_runtime_job(RuntimeJobKind::EnvironmentSetup)
            .unwrap();

        let tools = Tools {
            uv: environment.join("uv-fixture"),
            python310: environment.join("python/3.10/bin/python"),
            python312: environment.join("python/3.12/bin/python"),
        };
        let provisioned = ModelEnvironment {
            directory: environment,
            tools,
        };
        for path in [
            provisioned.uv(),
            provisioned.python("3.10"),
            provisioned.python("3.12"),
            &provisioned.hf_python(),
            &provisioned.hf_cli(),
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"tool fixture").unwrap();
        }
        fs::write(
            root.join(MANIFEST),
            serde_json::to_vec(&provisioned.tools).unwrap(),
        )
        .unwrap();
        assert!(ModelEnvironment::status(&root).ready);
        for job in jobs {
            assert!(
                core.validate_runtime_job(job)
                    .unwrap_err()
                    .to_string()
                    .contains("Sign in to Hugging Face")
            );
        }
        fs::remove_file(provisioned.python("3.12")).unwrap();
        assert!(!ModelEnvironment::status(&root).ready);
        drop(core);
        fs::remove_dir_all(directory).unwrap();
    }
}
