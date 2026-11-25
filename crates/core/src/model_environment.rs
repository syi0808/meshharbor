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
    uv_bootstrap::ensure_uv_tracked,
};

const DIRECTORY: &str = "model-environment";
const MANIFEST: &str = "model-environment/tools-v1.json";
const PYTHONS: [&str; 2] = ["3.10", "3.12"];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelEnvironmentStatus {
    pub ready: bool,
    pub python_versions: Vec<String>,
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
        ModelEnvironmentStatus {
            ready: Self::load(root).is_some(),
            python_versions: PYTHONS.map(str::to_owned).into(),
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
        let python_dir = directory.join("python");
        let environment = Self { directory, tools };
        (environment.tools.uv.is_absolute()
            && environment.uv().is_file()
            && [environment.python("3.10"), environment.python("3.12")]
                .into_iter()
                .all(|python| python.is_file() && python.starts_with(&python_dir))
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
        let uv = fs::canonicalize(ensure_uv_tracked(&root, execution)?)?;
        let command = |program: &Path| {
            let mut command = Command::new(program);
            command
                .env_clear()
                .current_dir(&directory)
                .stdin(Stdio::null())
                .env("PATH", env::var_os("PATH").unwrap_or_default())
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
        execution.stage("install_python")?;
        let mut install = command(&uv);
        install
            .args(["python", "install", "--no-bin"])
            .args(PYTHONS);
        execution.visible(
            install,
            "Could not install the shared managed Python interpreters",
        )?;
        let find = |version: &str| -> CoreResult<PathBuf> {
            let path = PathBuf::from(execution.capture(
                command(&uv).args([
                    "python",
                    "find",
                    "--no-project",
                    "--managed-python",
                    "--no-python-downloads",
                    version,
                ]),
                "Could not locate the managed Python interpreter",
            )?);
            let path = fs::canonicalize(path)?;
            if !path.starts_with(directory.join("python"))
                || !execution
                    .capture(
                        command(&path).arg("--version"),
                        "Could not start managed Python",
                    )?
                    .starts_with(&format!("Python {version}."))
            {
                return Err(CoreError::OperationUnavailable(
                    "uv returned an unexpected Python interpreter.".into(),
                ));
            }
            Ok(path)
        };
        let environment = Self {
            tools: Tools {
                python310: find("3.10")?,
                python312: find("3.12")?,
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
        managed_file_size(&root, MANIFEST)?;
        let staged = directory.join(format!(".tools-{}.json", Uuid::new_v4()));
        execution.begin_commit()?;
        fs::write(&staged, serde_json::to_vec(&environment.tools)?)?;
        let published = fs::rename(&staged, root.join(MANIFEST));
        if published.is_err() {
            let _ = fs::remove_file(staged);
        }
        published?;
        Ok(Self::status(&root))
    }
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
