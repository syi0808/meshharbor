use std::{
    env, fs, io,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    CoreError, CoreResult,
    huggingface_browser::PACKAGE,
    job_execution::JobExecution,
    model_storage::{managed_directory, managed_file_size, symlink_file},
    uv_bootstrap::{
        download_uv_tracked, is_executable, local_tool_dirs, managed_executable, uv_candidates,
        uv_executable_name,
    },
};

const DIRECTORY: &str = "model-environment";
const DOWNLOADS: &str = "model-environment/downloads";
const MANIFEST: &str = "model-environment/tools-v2.json";
const LEGACY_MANIFEST: &str = "model-environment/tools-v1.json";
const PYTHONS: [&str; 2] = ["3.10", "3.12"];

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ModelEnvironmentStatus {
    pub ready: bool,
    pub python_versions: Vec<String>,
    #[serde(default)]
    pub uv_found: bool,
    #[serde(default)]
    pub python_versions_found: Vec<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct Tools {
    uv: Option<PathBuf>,
    python310: Option<PathBuf>,
    python312: Option<PathBuf>,
    #[serde(default)]
    account_tools_ready: bool,
}

pub(crate) struct ModelEnvironment {
    directory: PathBuf,
    tools: Tools,
}

impl ModelEnvironment {
    pub(crate) fn status(root: &Path) -> ModelEnvironmentStatus {
        let tools = read_selected_tools(root).unwrap_or_default();
        ModelEnvironmentStatus {
            ready: tools.ready(&root.join(DIRECTORY)),
            python_versions: PYTHONS.map(str::to_owned).into(),
            uv_found: tools.uv.is_some(),
            python_versions_found: PYTHONS
                .into_iter()
                .filter(|version| tools.python(version).is_some())
                .map(str::to_owned)
                .collect(),
        }
    }

    pub(crate) fn load(root: &Path) -> Option<Self> {
        let directory = fs::canonicalize(root).ok()?.join(DIRECTORY);
        let tools = read_selected_tools(root)?;
        tools.ready(&directory).then_some(Self { directory, tools })
    }

    pub(crate) fn require(root: &Path) -> CoreResult<Self> {
        Self::load(root).ok_or_else(|| CoreError::OperationUnavailable(
            "Set up Python and uv in Studio → Settings → Environment first (CLI: meshharbor runtime environment-setup).".into(),
        ))
    }

    pub(crate) fn uv(&self) -> &Path {
        self.tools.uv.as_deref().expect("prepared uv executable")
    }

    pub(crate) fn python(&self, version: &str) -> &Path {
        self.tools
            .python(version)
            .as_deref()
            .expect("prepared Python interpreter")
    }

    pub(crate) fn hf_python(&self) -> PathBuf {
        hf_python(&self.directory)
    }

    pub(crate) fn hf_cli(&self) -> PathBuf {
        hf_cli(&self.directory)
    }

    pub(crate) fn detect(root: &Path) -> CoreResult<ModelEnvironmentStatus> {
        let root = prepare_directories(root)?;
        let previous = read_selected_tools(&root).unwrap_or_default();
        let legacy = read_tools(&root, LEGACY_MANIFEST).unwrap_or_default();
        let preferred_uv =
            env::var_os("MESHHARBOR_UV").and_then(|path| std::path::absolute(path).ok());
        let uv_sources = preferred_uv
            .into_iter()
            .chain(previous.uv.iter().cloned())
            .chain([managed_executable(&root.join(DOWNLOADS))])
            .chain(legacy.uv.iter().cloned())
            .chain([managed_executable(&root)])
            .chain(uv_candidates());
        detect_tools(&root, uv_sources, &previous, &legacy, |version| {
            python_candidates(&root, version).collect()
        })?;
        Ok(Self::status(&root))
    }

    pub(crate) fn prepare(
        root: &Path,
        execution: JobExecution<'_>,
    ) -> CoreResult<ModelEnvironmentStatus> {
        execution.stage("prepare_runtime")?;
        let root = prepare_directories(root)?;
        let mut tools = if let Some(tools) = read_selected_tools(&root) {
            tools
        } else {
            Self::detect(&root)?;
            read_selected_tools(&root).unwrap_or_default()
        };
        let directory = root.join(DIRECTORY);
        if tools.ready(&directory) {
            return Ok(Self::status(&root));
        }
        Self::prepare_missing(&root, &mut tools, execution)
    }

    fn prepare_missing(
        root: &Path,
        tools: &mut Tools,
        execution: JobExecution<'_>,
    ) -> CoreResult<ModelEnvironmentStatus> {
        let downloads = root.join(DOWNLOADS);
        let directory = root.join(DIRECTORY);
        if tools.python312.is_none() {
            tools.account_tools_ready = false;
        }
        let uv = match &tools.uv {
            Some(uv) => uv.clone(),
            None => download_uv_tracked(&downloads, execution)?,
        };
        tools.uv = Some(uv.clone());
        let find_downloaded = |version: &str| -> CoreResult<Option<PathBuf>> {
            let path = find_python(root, &uv, version, true, execution)?;
            Ok(path.filter(|path| {
                downloaded_python(&downloads, path) && python_is_usable(root, path, version)
            }))
        };
        execution.stage("prepare_python")?;
        for version in PYTHONS {
            if tools.python(version).is_none() {
                *tools.python_mut(version) = find_downloaded(version)?;
            }
        }
        let missing = PYTHONS
            .into_iter()
            .filter(|version| tools.python(version).is_none())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            execution.stage("install_python")?;
            let mut install = tool_command(root, &uv);
            install
                .args(["python", "install", "--no-bin"])
                .args(&missing);
            execution.visible(install, "Could not install missing Python interpreters")?;
            for version in missing {
                let path = find_downloaded(version)?.ok_or_else(|| {
                    CoreError::OperationUnavailable(format!(
                        "uv did not return a usable Python {version} interpreter."
                    ))
                })?;
                *tools.python_mut(version) = Some(path);
            }
        }
        write_tools(root, tools)?;
        if !tools.ready(&directory) {
            execution.stage("install_account_tools")?;
            let mut venv = tool_command(root, &uv);
            venv.args([
                "venv",
                "--no-project",
                "--allow-existing",
                "--no-python-downloads",
                "--python",
            ])
            .arg(
                tools
                    .python("3.12")
                    .as_ref()
                    .expect("prepared Python interpreter"),
            )
            .arg(directory.join("huggingface"));
            execution.visible(
                venv,
                "Could not prepare the shared Hugging Face environment",
            )?;

            let mut pip = tool_command(root, &uv);
            pip.args(["pip", "install", "--no-python-downloads", "--python"])
                .arg(hf_python(&directory))
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
                tool_command(root, &hf_cli(&directory))
                    .env("HF_HUB_OFFLINE", "1")
                    .arg("--help"),
                "Could not verify the shared Hugging Face tool",
            )?;
            tools.account_tools_ready = true;
            execution.begin_commit()?;
            write_tools(root, tools)?;
        }
        Ok(Self::status(root))
    }
}

impl Tools {
    fn python(&self, version: &str) -> &Option<PathBuf> {
        match version {
            "3.10" => &self.python310,
            "3.12" => &self.python312,
            _ => unreachable!("only configured Python versions can be used"),
        }
    }

    fn python_mut(&mut self, version: &str) -> &mut Option<PathBuf> {
        match version {
            "3.10" => &mut self.python310,
            "3.12" => &mut self.python312,
            _ => unreachable!("only configured Python versions can be used"),
        }
    }

    fn ready(&self, directory: &Path) -> bool {
        self.account_tools_ready
            && self.uv.is_some()
            && PYTHONS
                .into_iter()
                .all(|version| self.python(version).is_some())
            && hf_python(directory).is_file()
            && hf_cli(directory).is_file()
    }
}

fn prepare_directories(root: &Path) -> CoreResult<PathBuf> {
    let root = fs::canonicalize(root)?;
    for relative in [
        DIRECTORY,
        DOWNLOADS,
        "model-environment/downloads/python",
        "model-environment/home",
        "model-environment/uv-cache",
        "model-environment/huggingface",
    ] {
        managed_directory(&root, relative)?;
    }
    fs::create_dir_all(root.join(DOWNLOADS).join("python"))?;
    fs::create_dir_all(root.join(DIRECTORY).join("home"))?;
    Ok(root)
}

fn read_tools(root: &Path, manifest: &str) -> Option<Tools> {
    if !managed_file_size(root, manifest)
        .ok()
        .flatten()
        .is_some_and(|size| size < 8192)
    {
        return None;
    }
    serde_json::from_slice(&fs::read(root.join(manifest)).ok()?).ok()
}

fn read_selected_tools(root: &Path) -> Option<Tools> {
    let downloads = fs::canonicalize(root).ok()?.join(DOWNLOADS);
    let mut tools = read_tools(root, MANIFEST)?;
    if !managed_directory(root, DOWNLOADS).ok()? {
        return Some(Tools::default());
    }
    for path in [&mut tools.uv, &mut tools.python310, &mut tools.python312] {
        if path.as_ref().is_some_and(|path| {
            !path.starts_with(&downloads)
                || path
                    .components()
                    .any(|component| component == Component::ParentDir)
                || !is_executable(path)
        }) {
            *path = None;
        }
    }
    Some(tools)
}

fn write_tools(root: &Path, tools: &Tools) -> CoreResult<()> {
    managed_file_size(root, MANIFEST)?;
    let staged = root
        .join(DIRECTORY)
        .join(format!(".tools-{}.json", Uuid::new_v4()));
    fs::write(&staged, serde_json::to_vec(tools)?)?;
    if let Err(error) = fs::rename(&staged, root.join(MANIFEST)) {
        let _ = fs::remove_file(staged);
        return Err(error.into());
    }
    Ok(())
}

fn tool_command(root: &Path, program: &Path) -> Command {
    let directory = root.join(DIRECTORY);
    let mut command = Command::new(program);
    command
        .env_clear()
        .current_dir(&directory)
        .stdin(Stdio::null())
        .env(
            "PATH",
            env::join_paths(local_tool_dirs()).unwrap_or_default(),
        )
        .env("HOME", directory.join("home"))
        .env("UV_PYTHON_INSTALL_DIR", root.join(DOWNLOADS).join("python"))
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
}

fn find_python(
    root: &Path,
    uv: &Path,
    version: &str,
    managed: bool,
    execution: JobExecution<'_>,
) -> CoreResult<Option<PathBuf>> {
    let mut command = tool_command(root, uv);
    if !managed {
        command
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
                command.env(key, value);
            }
        }
    }
    command.args([
        "python",
        "find",
        "--system",
        "--offline",
        "--no-project",
        "--no-python-downloads",
    ]);
    if managed {
        command.arg("--managed-python");
    }
    command.arg(format!("cpython@{version}"));
    match execution.capture(&mut command, "Could not locate Python") {
        Ok(output) => {
            let path = PathBuf::from(output);
            if !path.is_absolute() || !is_executable(&path) {
                return Ok(None);
            }
            Ok(if managed {
                fs::canonicalize(path).ok()
            } else {
                Some(path)
            })
        }
        Err(CoreError::Cancelled) => Err(CoreError::Cancelled),
        Err(_) => Ok(None),
    }
}

fn downloaded_python(downloads: &Path, path: &Path) -> bool {
    path.starts_with(downloads.join("python"))
}

fn detect_tools(
    root: &Path,
    uv_sources: impl IntoIterator<Item = PathBuf>,
    previous: &Tools,
    legacy: &Tools,
    fallback_python: impl Fn(&str) -> Vec<PathBuf>,
) -> CoreResult<()> {
    let downloads = root.join(DOWNLOADS);
    let execution = JobExecution::default();
    let uv_usable = |path: &Path| {
        execution
            .capture(
                tool_command(root, path).arg("--version"),
                "Could not run uv",
            )
            .is_ok_and(|output| output.starts_with("uv "))
            && execution
                .capture(
                    tool_command(root, path).args([
                        "python",
                        "install",
                        "--offline",
                        "--no-bin",
                        "--help",
                    ]),
                    "uv does not support private Python installs",
                )
                .is_ok()
    };
    let uv = select_tool(
        uv_sources,
        &downloads,
        &downloads.join(uv_executable_name()),
        uv_usable,
    );
    let mut tools = Tools {
        uv,
        ..Tools::default()
    };
    for version in PYTHONS {
        let alias = downloads.join(python_binary_name(&format!("python{version}")));
        let usable = |path: &Path| python_is_usable(root, path, version);
        let mut sources = previous
            .python(version)
            .iter()
            .chain(legacy.python(version))
            .cloned();
        let selected = select_tool(&mut sources, &downloads, &alias, usable)
            .or_else(|| {
                let uv = tools.uv.as_ref()?;
                let downloaded = find_python(root, uv, version, true, execution)
                    .ok()
                    .flatten()
                    .filter(|path| downloaded_python(&downloads, path));
                let external = std::iter::once_with(|| {
                    find_python(root, uv, version, false, execution)
                        .ok()
                        .flatten()
                })
                .flatten();
                select_tool(
                    downloaded.into_iter().chain(external),
                    &downloads,
                    &alias,
                    usable,
                )
            })
            .or_else(|| select_tool(fallback_python(version), &downloads, &alias, usable));
        *tools.python_mut(version) = selected;
    }
    let directory = root.join(DIRECTORY);
    tools.account_tools_ready = (previous.account_tools_ready || legacy.uv.is_some())
        && tools.python("3.12").is_some()
        && hf_python(&directory).is_file()
        && hf_cli(&directory).is_file()
        && execution
            .capture(
                tool_command(root, &hf_cli(&directory))
                    .env("HF_HUB_OFFLINE", "1")
                    .arg("--help"),
                "Could not verify the shared Hugging Face tool",
            )
            .is_ok();
    write_tools(root, &tools)
}

fn select_tool(
    candidates: impl IntoIterator<Item = PathBuf>,
    downloads: &Path,
    alias: &Path,
    usable: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    for candidate in candidates {
        if !candidate.is_absolute() || !is_executable(&candidate) || !usable(&candidate) {
            continue;
        }
        if candidate.starts_with(downloads) {
            return Some(candidate);
        }
        if link_executable(&candidate, alias).is_ok() {
            if usable(alias) {
                return Some(alias.to_path_buf());
            }
            let _ = fs::remove_file(alias);
        }
    }
    None
}

fn link_executable(target: &Path, link: &Path) -> io::Result<()> {
    match fs::symlink_metadata(link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if fs::read_link(link)? == target {
                return Ok(());
            }
            fs::remove_file(link)?;
        }
        Ok(_) => return Err(io::ErrorKind::AlreadyExists.into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    symlink_file(target, link)
}

fn python_candidates(root: &Path, version: &str) -> impl Iterator<Item = PathBuf> {
    let names = [
        python_binary_name(&format!("python{version}")),
        python_binary_name("python3"),
        python_binary_name("python"),
    ];
    let install_roots = [
        Some(root.join(DOWNLOADS).join("python")),
        Some(root.join(DIRECTORY).join("python")),
        env::var_os("UV_PYTHON_INSTALL_DIR").map(PathBuf::from),
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .map(|directory| directory.join("uv/python")),
    ];
    let prefix = format!("cpython-{version}.");
    let interpreter = if cfg!(windows) {
        PathBuf::from("python.exe")
    } else {
        PathBuf::from(format!("bin/python{version}"))
    };
    let managed = install_roots
        .into_iter()
        .flatten()
        .filter(|path| path.is_absolute())
        .filter_map(|path| fs::read_dir(path).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter(move |entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(move |entry| entry.path().join(&interpreter));
    let system = local_tool_dirs()
        .into_iter()
        .filter(|path| !(cfg!(windows) && path.ends_with("WindowsApps")))
        .flat_map(move |directory| {
            names
                .clone()
                .into_iter()
                .map(move |name| directory.join(name))
        })
        .filter(|path| !(cfg!(target_os = "macos") && path == Path::new("/usr/bin/python3")))
        .chain(
            env::var_os("LOCALAPPDATA")
                .filter(|_| cfg!(windows))
                .map(|base| {
                    PathBuf::from(base).join(format!(
                        "Programs/Python/Python{}/python.exe",
                        version.replace('.', "")
                    ))
                }),
        );
    managed.chain(system)
}

fn python_is_usable(root: &Path, path: &Path, version: &str) -> bool {
    JobExecution::default().capture(
        tool_command(root, path).args(["-I", "-c",
            "import platform, struct, sys; print(sys.implementation.name, '%d.%d' % sys.version_info[:2], platform.machine().lower(), struct.calcsize('P'), sys.prefix == sys.base_prefix)"]),
        "Could not inspect Python",
    ).is_ok_and(|output| {
        let arch = env::consts::ARCH;
        let mut fields = output.split_whitespace();
        fields.next() == Some("cpython")
            && fields.next() == Some(version)
            && fields.next().is_some_and(|machine| machine == arch || matches!((machine, arch),
                ("amd64", "x86_64") | ("arm64", "aarch64") | ("i386" | "i686", "x86")))
            && fields.next().is_some_and(|width| width.parse::<usize>() == Ok(std::mem::size_of::<usize>()))
            && fields.next() == Some("True")
    })
}

fn python_binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn hf_python(directory: &Path) -> PathBuf {
    directory.join("huggingface").join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    })
}

fn hf_cli(directory: &Path) -> PathBuf {
    directory.join("huggingface").join(if cfg!(windows) {
        "Scripts/hf.exe"
    } else {
        "bin/hf"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OperationCore, RuntimeJobKind};

    #[cfg(unix)]
    #[test]
    fn startup_links_compatible_tools_and_explicit_setup_downloads_only_missing_python() {
        use std::os::unix::fs::PermissionsExt;

        let root = env::temp_dir().join(format!("meshharbor-python-reuse-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let root = prepare_directories(&root).unwrap();
        let directory = root.join(DIRECTORY);
        let downloads = root.join(DOWNLOADS);
        let local = root.join("local-bin");
        fs::create_dir(&local).unwrap();
        let put_script = |path: &Path, content: &str| {
            fs::write(path, content).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        };
        let python_script = |version| {
            format!(
                "#!/bin/sh\necho 'cpython {version} {} {} True'\n",
                env::consts::ARCH,
                std::mem::size_of::<usize>(),
            )
        };
        let python312 = local.join("python3.12");
        put_script(&python312, &python_script("3.12"));
        put_script(&local.join("python3.10"), &python_script("3.13"));
        put_script(&local.join("downloaded-python3.10"), &python_script("3.10"));
        let uv = local.join("uv");
        put_script(
            &uv,
            r#"#!/bin/sh
set -eu
local_bin="$PWD/../local-bin"
for argument; do request="$argument"; done
case "$*" in
    --version) echo "uv fixture" ;;
    "python install --offline --no-bin --help") exit 0 ;;
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
        cp "$local_bin/downloaded-python3.10" "$path/python3.10"
        ;;
    "venv "*)
        test "$6" = "$PWD/downloads/python3.12"
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
        let detect = || {
            detect_tools(
                &root,
                [uv.clone()],
                &Tools::default(),
                &Tools::default(),
                |_| Vec::new(),
            )
            .unwrap();
        };
        detect();
        let status = ModelEnvironment::status(&root);
        assert!(status.uv_found);
        assert_eq!(status.python_versions_found, ["3.12"]);
        assert!(!status.ready);
        assert!(!directory.join("installs").exists());
        assert!(!hf_cli(&directory).exists());

        let uv_link = downloads.join("uv");
        let python_link = downloads.join("python3.12");
        assert_eq!(fs::canonicalize(&uv_link).unwrap(), uv);
        assert_eq!(fs::canonicalize(&python_link).unwrap(), python312);
        assert!(fs::symlink_metadata(&python_link).unwrap().is_symlink());

        put_script(&local.join("python3.10"), &python_script("3.10"));
        assert_eq!(
            ModelEnvironment::status(&root).python_versions_found,
            ["3.12"]
        );
        for _ in 0..2 {
            let status = ModelEnvironment::prepare(&root, Default::default()).unwrap();
            assert!(status.ready);
            let environment = ModelEnvironment::require(&root).unwrap();
            assert_eq!(environment.uv(), uv_link);
            assert_eq!(environment.python("3.12"), python_link);
            assert!(
                environment
                    .python("3.10")
                    .starts_with(downloads.join("python"))
            );
        }
        assert_eq!(
            fs::read_to_string(directory.join("installs")).unwrap(),
            "python install --no-bin 3.10\n"
        );
        fs::remove_file(python312).unwrap();
        assert!(!ModelEnvironment::status(&root).ready);
        assert!(fs::symlink_metadata(&python_link).unwrap().is_symlink());
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
            uv: Some(root.join(DOWNLOADS).join("uv-fixture")),
            python310: Some(root.join(DOWNLOADS).join("python/3.10/bin/python")),
            python312: Some(root.join(DOWNLOADS).join("python/3.12/bin/python")),
            account_tools_ready: true,
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
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
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
