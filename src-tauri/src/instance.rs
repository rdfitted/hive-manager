//! Debug-only fixture instances. Resolve and apply before any application threads exist.
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

pub const DIR: &str = "HIVE_MANAGER_INSTANCE_DIR";
pub const PORT: &str = "HIVE_MANAGER_INSTANCE_PORT";
pub const CDP_PORT: &str = "HIVE_MANAGER_INSTANCE_CDP_PORT";
pub type Environment = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildProfile {
    Debug,
    Release,
}

impl BuildProfile {
    pub fn current() -> Self {
        if cfg!(debug_assertions) {
            Self::Debug
        } else {
            Self::Release
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub root: PathBuf,
    pub port: u16,
    pub cdp_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instance {
    Normal,
    Isolated(Settings),
}

static INSTANCE: OnceLock<Instance> = OnceLock::new();

pub fn settings() -> Option<&'static Settings> {
    match INSTANCE.get() {
        Some(Instance::Isolated(settings)) => Some(settings),
        _ => None,
    }
}

pub fn is_isolated() -> bool {
    settings().is_some()
}

fn absolute(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("Instance paths must be absolute without parent components".into());
    }
    Ok(())
}

// Resolve existing ancestors too, so a missing descendant cannot disguise a junction.
fn resolved_path(path: &Path) -> Result<PathBuf, String> {
    absolute(path)?;
    if path.exists() {
        return path.canonicalize().map_err(|e| e.to_string());
    }
    let parent = path.parent().ok_or("Path has no parent")?;
    Ok(resolved_path(parent)?.join(path.file_name().ok_or("Path has no name")?))
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn port(value: Option<&String>) -> Result<u16, String> {
    let number = value
        .ok_or("Instance API port is required")?
        .parse::<u16>()
        .map_err(|_| "Invalid instance port")?;
    if number < 1024 || number == 18800 {
        return Err("Instance port must be >=1024 and not 18800".into());
    }
    Ok(number)
}

/// Filesystem reads are limited to canonicalization/marker checks; no environment mutation.
pub fn resolve(env: &Environment, profile: BuildProfile) -> Result<Instance, String> {
    if profile == BuildProfile::Release
        || ![DIR, PORT, CDP_PORT]
            .iter()
            .any(|key| env.contains_key(*key))
    {
        return Ok(Instance::Normal);
    }
    let requested = PathBuf::from(env.get(DIR).ok_or("Instance directory is required")?);
    absolute(&requested)?;
    let root = requested
        .canonicalize()
        .map_err(|_| "Instance directory must already exist")?;
    if root.parent().is_none() || !root.join(".hive-instance").is_file() {
        return Err(
            "Instance directory requires .hive-instance marker and cannot be a drive root".into(),
        );
    }
    for key in ["HOME", "USERPROFILE"] {
        if let Some(home) = env.get(key) {
            let home = resolved_path(Path::new(home))?;
            if root == home || home.starts_with(&root) {
                return Err("Instance cannot contain the real home".into());
            }
        }
    }
    let mut protected = Vec::new();
    if let Some(path) = env.get("APPDATA") {
        protected.push(PathBuf::from(path).join("hive-manager"));
    }
    if let Some(path) = env.get("HOME") {
        protected.push(PathBuf::from(path).join(".config/hive-manager"));
    }
    if let Some(path) = env.get("LOCALAPPDATA") {
        protected.push(PathBuf::from(path).join("com.rduff.hive-manager"));
    }
    if let Some(path) = env.get("WEBVIEW2_USER_DATA_FOLDER") {
        protected.push(PathBuf::from(path));
    }
    for protected in protected {
        if overlaps(&root, &resolved_path(&protected)?) {
            return Err("Instance overlaps real data or webview profile".into());
        }
    }
    let api_port = port(env.get(PORT))?;
    let cdp_port = env
        .get(CDP_PORT)
        .map(|value| port(Some(value)))
        .transpose()?;
    if cdp_port == Some(api_port) {
        return Err("Instance API and CDP ports must differ".into());
    }
    Ok(Instance::Isolated(Settings {
        root,
        port: api_port,
        cdp_port,
    }))
}

impl Settings {
    pub fn data_dir(&self) -> PathBuf {
        if cfg!(windows) {
            self.root.join("appdata/hive-manager")
        } else {
            self.root.join("home/.config/hive-manager")
        }
    }

    pub fn environment(&self) -> Environment {
        let mut env = Environment::new();
        for (key, relative) in [
            ("APPDATA", "appdata"),
            ("LOCALAPPDATA", "localappdata"),
            ("USERPROFILE", "home"),
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "home/.config"),
            ("XDG_DATA_HOME", "home/.local/share"),
            ("XDG_CACHE_HOME", "home/.cache"),
            ("TEMP", "temp"),
            ("TMP", "temp"),
            ("TMPDIR", "temp"),
            ("HIVE_WIKI_ROOT", "home/.ai-docs/wiki"),
            ("CODEX_HOME", "home/.codex"),
            ("CLAUDE_CONFIG_DIR", "home/.claude"),
            ("WEBVIEW2_USER_DATA_FOLDER", "webview"),
        ] {
            env.insert(
                key.into(),
                self.root.join(relative).to_string_lossy().into_owned(),
            );
        }
        env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        env.insert(
            "GIT_CONFIG_GLOBAL".into(),
            self.root
                .join("home/.gitconfig")
                .to_string_lossy()
                .into_owned(),
        );
        env.insert(
            "GIT_CONFIG_SYSTEM".into(),
            self.root
                .join("home/.gitconfig-system")
                .to_string_lossy()
                .into_owned(),
        );
        env.insert(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS".into(),
            self.cdp_port
                .map(|port| {
                    format!("--remote-debugging-port={port} --remote-debugging-address=127.0.0.1")
                })
                .unwrap_or_default(),
        );
        env
    }

    pub fn contains(&self, path: &Path) -> Result<(), String> {
        if !resolved_path(path)?.starts_with(&self.root) {
            return Err("Fixture path escapes instance directory".into());
        }
        Ok(())
    }
}

fn scrub_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key.starts_with("HIVE_")
        || key.starts_with("CLAUDE_CODE_")
        || key.starts_with("GIT_")
        || key.starts_with("WEBVIEW2_")
        || ["HOMEDRIVE", "HOMEPATH", "ZDOTDIR", "ENV", "BASH_ENV"].contains(&key.as_str())
}

fn reject_links(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let linked = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        linked || metadata.file_attributes() & 0x400 != 0
    };
    if linked {
        return Err("Instance tree cannot contain symlinks or reparse points".into());
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
            reject_links(&entry.map_err(|e| e.to_string())?.path())?;
        }
    }
    Ok(())
}

/// Called exactly once, first in run(), while the process is still single threaded.
pub fn bootstrap() -> &'static Instance {
    INSTANCE.get_or_init(|| {
        let env = std::env::vars_os()
            .map(|(key, value)| {
                (
                    if cfg!(windows) {
                        key.to_string_lossy().to_ascii_uppercase()
                    } else {
                        key.to_string_lossy().into_owned()
                    },
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect();
        let result = (|| {
            let instance = resolve(&env, BuildProfile::current())?;
            if let Instance::Isolated(settings) = &instance {
                reject_links(&settings.root)?;
                for folder in [
                    "appdata",
                    "localappdata",
                    "home",
                    "temp",
                    "webview",
                    "projects",
                ] {
                    std::fs::create_dir_all(settings.root.join(folder))
                        .map_err(|e| e.to_string())?;
                }
                for key in std::env::vars_os().map(|(key, _)| key) {
                    if scrub_key(&key.to_string_lossy()) {
                        std::env::remove_var(key);
                    }
                }
                for (key, value) in settings.environment() {
                    std::env::set_var(key, value);
                }
                std::env::set_current_dir(&settings.root).map_err(|e| e.to_string())?;
                let data_dir = crate::storage::SessionStorage::get_app_data_dir()
                    .map_err(|e| e.to_string())?;
                settings.contains(&data_dir)?;
                if resolved_path(&data_dir)? != resolved_path(&settings.data_dir())? {
                    return Err(
                        "Storage did not resolve to the instance data directory".to_string()
                    );
                }
            }
            Ok::<_, String>(instance)
        })();
        result.unwrap_or_else(|error| {
            eprintln!("Isolated instance refused: {error}");
            std::process::exit(78)
        })
    })
}

pub fn refuse(operation: &str) -> Result<(), String> {
    if is_isolated() {
        Err(format!("Isolated instance: {operation} is disabled"))
    } else {
        Ok(())
    }
}

impl Settings {
    pub fn api_base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn init_script(&self) -> String {
        format!("Object.defineProperty(window, '__HIVE_INSTANCE__', {{ value: Object.freeze({{isolated:true,apiBase:{}}}), writable:false, configurable:false }});",
            serde_json::to_string(&self.api_base()).expect("API base is serializable"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Environment) {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".hive-instance"), "fixture").unwrap();
        let env = Environment::from([
            (DIR.into(), temp.path().to_string_lossy().into_owned()),
            (PORT.into(), "18891".into()),
        ]);
        (temp, env)
    }

    #[test]
    fn instance_refuses_live_data_and_profile() {
        let (temp, mut env) = fixture();
        env.insert(
            "APPDATA".into(),
            temp.path()
                .join("real-appdata")
                .to_string_lossy()
                .into_owned(),
        );
        let data = PathBuf::from(env.get("APPDATA").unwrap()).join("hive-manager");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join(".hive-instance"), "fixture").unwrap();
        env.insert(DIR.into(), data.to_string_lossy().into_owned());
        assert!(resolve(&env, BuildProfile::Debug).is_err());
        env.insert(DIR.into(), temp.path().to_string_lossy().into_owned());
        env.remove("APPDATA");
        env.insert(
            "WEBVIEW2_USER_DATA_FOLDER".into(),
            temp.path().to_string_lossy().into_owned(),
        );
        assert!(resolve(&env, BuildProfile::Debug).is_err());
    }

    #[test]
    fn instance_refuses_live_port_partial_settings_and_invalid_roots() {
        let (temp, mut env) = fixture();
        assert!(matches!(
            resolve(&env, BuildProfile::Debug),
            Ok(Instance::Isolated(_))
        ));
        for value in ["18800", "0", "1023", "65536", ""] {
            env.insert(PORT.into(), value.into());
            assert!(resolve(&env, BuildProfile::Debug).is_err(), "port {value}");
        }
        env.insert(PORT.into(), "18891".into());
        env.insert(CDP_PORT.into(), "18891".into());
        assert!(resolve(&env, BuildProfile::Debug).is_err());
        env.remove(CDP_PORT);
        env.remove(PORT);
        assert!(resolve(&env, BuildProfile::Debug).is_err());
        env.clear();
        env.insert(PORT.into(), "18891".into());
        assert!(resolve(&env, BuildProfile::Debug).is_err());
        env.insert(
            DIR.into(),
            temp.path().join("..").to_string_lossy().into_owned(),
        );
        assert!(resolve(&env, BuildProfile::Debug).is_err());
        env.insert(DIR.into(), temp.path().to_string_lossy().into_owned());
        std::fs::remove_file(temp.path().join(".hive-instance")).unwrap();
        assert!(resolve(&env, BuildProfile::Debug).is_err());
    }

    #[test]
    fn instance_release_ignores_settings() {
        let (_, mut env) = fixture();
        env.insert(PORT.into(), "18800".into());
        env.insert(CDP_PORT.into(), "invalid".into());
        assert_eq!(
            resolve(&env, BuildProfile::Release).unwrap(),
            Instance::Normal
        );
        env.remove(DIR);
        assert_eq!(
            resolve(&env, BuildProfile::Release).unwrap(),
            Instance::Normal
        );
    }

    #[test]
    fn instance_startup_wiring_is_fail_closed() {
        let source =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
                .unwrap()
                .replace("\r\n", "\n");
        let run = source.split_once("pub fn run() {").unwrap().1.trim_start();
        assert!(
            run.starts_with("instance::bootstrap();"),
            "bootstrap must precede tracing/threads"
        );
        assert!(source.contains("config.api.enabled = true;"));
        assert!(source.contains("config.api.port = settings.port;"));
        let bind_error = source
            .split_once("tracing::error!(\"HTTP server error: {}\", e);")
            .unwrap()
            .1;
        assert!(bind_error
            .trim_start()
            .starts_with("if instance::is_isolated() {"));
        assert!(bind_error
            .split_once('}')
            .unwrap()
            .0
            .contains("std::process::exit(78)"));
        assert!(source.contains("window.focus = false;"));
        assert!(source.contains("window.data_directory = Some(settings.root.join(\"webview\"));"));
    }

    #[test]
    fn instance_child_roots_and_bootstrap() {
        if std::env::var_os("HM_INSTANCE_TEST_CHILD").is_some() {
            bootstrap();
            let settings = settings().expect("child isolation");
            let base = crate::storage::SessionStorage::get_app_data_dir().unwrap();
            let storage = crate::storage::SessionStorage::new_with_base(base).unwrap();
            assert_eq!(
                resolved_path(storage.base_dir()).unwrap(),
                resolved_path(&settings.data_dir()).unwrap()
            );
            for (key, relative) in [
                ("APPDATA", "appdata"),
                ("LOCALAPPDATA", "localappdata"),
                ("HOME", "home"),
                ("USERPROFILE", "home"),
                ("TMPDIR", "temp"),
                ("TEMP", "temp"),
                ("TMP", "temp"),
                ("WEBVIEW2_USER_DATA_FOLDER", "webview"),
            ] {
                assert_eq!(
                    PathBuf::from(std::env::var(key).unwrap()),
                    settings.root.join(relative),
                    "{key}"
                );
            }
            for path in [
                std::env::temp_dir(),
                crate::cli::agent_store::store_root(),
                crate::wiki::resolve_wiki_root(None),
                PathBuf::from(std::env::var("USERPROFILE").unwrap()).join(".codex/sessions"),
                PathBuf::from(std::env::var("HOME").unwrap()).join(".claude/projects"),
                PathBuf::from(std::env::var("CODEX_HOME").unwrap()),
                PathBuf::from(std::env::var("CLAUDE_CONFIG_DIR").unwrap()),
            ] {
                settings.contains(&path).unwrap();
            }
            for key in [
                "HIVE_AGENT_TOKEN",
                "HIVE_SESSION_ID",
                "CLAUDE_CODE_CHILD_SESSION",
                "GIT_DIR",
                "GIT_WORK_TREE",
            ] {
                assert!(std::env::var_os(key).is_none(), "inherited {key}");
            }
            let manager = crate::pty::PtyManager::new();
            let error = manager
                .create_session(
                    "synthetic-agent".into(),
                    crate::pty::AgentRole::Queen,
                    "missing-instance-test-command",
                    &[],
                    None,
                    80,
                    24,
                )
                .unwrap_err();
            assert!(error.to_string().contains("Isolated instance"));
            assert!(manager.resize("synthetic-agent", 80, 24).is_ok());
            use crate::runtime::RuntimeAdapter;
            let runtime = crate::runtime::LocalProcessRuntime::new();
            let spec = crate::runtime::LaunchSpec::new("missing-instance-test-command");
            assert!(runtime
                .launch(&spec)
                .unwrap_err()
                .to_string()
                .contains("Isolated instance"));
            let controller = crate::session::SessionController::new(std::sync::Arc::new(
                parking_lot::RwLock::new(manager),
            ));
            let project = settings.root.join("must-not-be-created");
            assert!(controller
                .launch_hive(
                    project.clone(),
                    1,
                    "missing-instance-test-command",
                    None,
                    None,
                    None
                )
                .unwrap_err()
                .contains("Isolated instance"));
            assert!(!project.exists());
            let health = tokio::runtime::Runtime::new().unwrap().block_on(
                crate::cli::health::CliHealthRegistry::check_all(settings.port),
            );
            assert!(!health.clis.is_empty());
            assert!(health.clis.iter().all(|cli| !cli.resolved
                && cli.bin_path.is_none()
                && cli.detail.contains("disabled")));

            assert!(settings.init_script().contains(&settings.api_base()));
            assert!(!settings.init_script().contains(":18800"));
            assert!(is_isolated());
            return;
        }
        let (temp, env) = fixture();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "instance::tests::instance_child_roots_and_bootstrap",
                "--nocapture",
            ])
            .envs(env)
            .env("HM_INSTANCE_TEST_CHILD", "1")
            .env("HIVE_AGENT_TOKEN", "synthetic-token")
            .env("HIVE_SESSION_ID", "synthetic-session")
            .env("CLAUDE_CODE_CHILD_SESSION", "synthetic")
            .env("GIT_DIR", temp.path().join("forbidden-git"))
            .env("GIT_WORK_TREE", temp.path().join("forbidden-tree"));
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!is_isolated(), "parent must stay normal");
    }

    fn source(relative: &str) -> String {
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative))
            .unwrap()
            .replace("\r\n", "\n")
    }

    fn first_statement<'a>(source: &'a str, name: &str) -> &'a str {
        source
            .split_once(&format!("fn {name}("))
            .unwrap()
            .1
            .split_once(" {\n")
            .unwrap()
            .1
            .trim_start()
    }

    #[test]
    fn instance_spawn_guards_precede_side_effects() {
        let manager = source("src/pty/manager.rs");
        assert!(first_statement(&manager, "create_session").starts_with("crate::instance::refuse("));
        // Windows tests replace LocalPtyRuntime with a stub: inspect the production file.
        for path in ["src/runtime/local_pty.rs", "src/runtime/local_process.rs"] {
            let runtime = source(path);
            assert!(
                first_statement(&runtime, "launch").starts_with("crate::instance::refuse("),
                "{path}"
            );
        }
        let controller = source("src/session/controller.rs");
        for name in [
            "launch_hive",
            "launch_solo",
            "launch_hive_v2",
            "launch_research",
            "launch_fusion",
            "launch_debate",
            "launch_swarm",
            "launch_evaluator",
            "launch_prince",
            "add_worker",
            "add_worker_for_plan_task",
            "add_qa_worker",
            "add_planner",
            "reserve_add_worker",
            "continue_after_planning_checked",
            "spawn_next_worker",
            "spawn_debate_round",
            "spawn_fusion_judge",
            "spawn_debate_judge",
            "spawn_launch_evaluator_agents",
        ] {
            assert!(
                first_statement(&controller, name).starts_with("crate::instance::refuse("),
                "{name}"
            );
        }
    }

    #[test]
    fn instance_probe_sweep_preview_and_updater_guards() {
        for (path, name) in [
            ("src/cli/health.rs", "check_all"),
            ("src/cli/agent_store.rs", "cleanup_stale_stores"),
        ] {
            let source = source(path);
            assert!(
                first_statement(&source, name).starts_with("if crate::instance::is_isolated()"),
                "{path}"
            );
        }
        let preview = source("src/preview/mod.rs");
        assert!(first_statement(&preview, "open_preview_window")
            .starts_with("crate::instance::refuse("));
        let lib = source("src/lib.rs");
        let plugins = lib
            .split_once("if !instance::is_isolated() {\n        builder = builder")
            .unwrap()
            .1
            .split_once("\n    }")
            .unwrap()
            .0;
        assert!(plugins.contains("tauri_plugin_updater::Builder::new().build()"));
        assert!(plugins.contains("tauri_plugin_process::init()"));
        let macos = lib
            .split_once("cli::env_path::initialize_macos_cli_environment();")
            .unwrap()
            .0;
        assert!(macos.trim_end().ends_with("if !instance::is_isolated() {"));
    }

    fn forbidden_startup_code(text: &str) -> bool {
        let text = text.to_ascii_lowercase().replace("\\\\", "\\");
        [
            ["tauri_plugin_", "autostart"].concat(),
            ["tauri-plugin-", "autostart"].concat(),
            ["\\currentversion", "\\run"].concat(),
            ["regset", "value"].concat(),
        ]
        .iter()
        .any(|pattern| text.contains(pattern))
    }

    fn scan_startup_code(path: &Path) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                scan_startup_code(&path);
            } else if path
                .extension()
                .is_some_and(|ext| ["rs", "ts", "svelte"].iter().any(|allowed| ext == *allowed))
            {
                assert!(
                    !forbidden_startup_code(&std::fs::read_to_string(&path).unwrap()),
                    "startup registration in {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn instance_no_autostart_source_guard() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        scan_startup_code(&manifest.join("src"));
        scan_startup_code(&manifest.join("../src"));
    }
    #[test]
    fn instance_webview_injection_precedes_scripts() {
        let lib = source("src/lib.rs");
        let plugin = lib
            .split_once(
                "if let Some(settings) = instance::settings() {\n        builder = builder.plugin(",
            )
            .unwrap()
            .1
            .split_once("\n    }")
            .unwrap()
            .0;
        assert!(plugin.contains(".js_init_script(settings.init_script())"));
    }
}
