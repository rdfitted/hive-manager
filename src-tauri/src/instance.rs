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
    #[cfg(windows)]
    if let Some(Component::Prefix(prefix)) = path.components().next() {
        if !matches!(
            prefix.kind(),
            std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
        ) {
            return Err("Network and device paths are disabled for isolated instances".into());
        }
    }
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

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureState {
    pub version: u32,
    pub sessions: Vec<FixtureSession>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureSession {
    pub session_id: String,
    pub agents: Vec<FixtureAgent>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureAgent {
    pub agent_id: String,
    pub status: crate::pty::AgentStatus,
    pub work_state: Option<crate::session::work_state::WorkState>,
    pub heartbeat: Option<FixtureHeartbeat>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureHeartbeat {
    pub status: String,
    pub summary: Option<String>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
}

static FIXTURE_STATE: OnceLock<FixtureState> = OnceLock::new();

/// Only startup after successful debug bootstrap can enter the fixture seam.
pub(crate) fn load_fixtures(
    controller: &crate::session::SessionController,
    storage: &crate::storage::SessionStorage,
) -> Result<(), String> {
    let Some(settings) = settings() else {
        return Ok(());
    };
    let path = settings.root.join("fixture-state.json");
    if !path.exists() {
        return Ok(());
    }
    let state: FixtureState =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Invalid fixture overlay: {e}"))?;
    if state.version != 1 {
        return Err("Unsupported fixture overlay version".into());
    }
    settings.contains(storage.base_dir())?;
    let mut sessions = std::collections::HashSet::new();
    // Validate every session before resume can start a watcher or inspect a project.
    for fixture in &state.sessions {
        if fixture.session_id.is_empty()
            || fixture.session_id.contains(['/', '\\'])
            || fixture.session_id.contains("..")
            || !sessions.insert(&fixture.session_id)
        {
            return Err("Invalid or duplicate fixture session ID".into());
        }
        let persisted = storage
            .load_session(&fixture.session_id)
            .map_err(|e| e.to_string())?;
        if persisted.id != fixture.session_id {
            return Err("Fixture session ID mismatch".into());
        }
        settings.contains(Path::new(&persisted.project_path))?;
        if let Some(path) = &persisted.worktree_path {
            settings.contains(Path::new(path))?;
        }
        for agent in &persisted.agents {
            if let Some(path) = &agent.transcript_cwd {
                settings.contains(Path::new(path))?;
            }
        }
        let mut agents = std::collections::HashSet::new();
        for agent in &fixture.agents {
            if !agents.insert(&agent.agent_id)
                || !persisted
                    .agents
                    .iter()
                    .any(|known| known.id == agent.agent_id)
            {
                return Err("Unknown or duplicate fixture agent ID".into());
            }
        }
    }
    FIXTURE_STATE
        .set(state)
        .map_err(|_| "Fixture overlay already loaded".to_string())?;
    for fixture in &FIXTURE_STATE
        .get()
        .expect("installed fixture overlay")
        .sessions
    {
        controller.resume_session(&fixture.session_id)?;
        controller.apply_instance_fixture(fixture)?;
    }
    Ok(())
}

pub(crate) fn project_fixture_work_states(session: &mut crate::session::Session) {
    if !is_isolated() {
        return;
    }
    let Some(fixture) = FIXTURE_STATE.get().and_then(|state| {
        state
            .sessions
            .iter()
            .find(|fixture| fixture.session_id == session.id)
    }) else {
        return;
    };
    for agent in &mut session.agents {
        if let Some(work_state) = fixture
            .agents
            .iter()
            .find(|fixture| fixture.agent_id == agent.id)
            .and_then(|fixture| fixture.work_state)
        {
            agent.work_state = Some(work_state);
        }
    }
}

/// Every caller-supplied or stored filesystem target crosses this boundary.
pub(crate) fn guard_path(path: &Path) -> Result<(), String> {
    let Some(settings) = settings() else {
        return Ok(());
    };
    settings
        .contains(path)
        .map_err(|error| format!("Isolated instance: {error}"))
}

pub(crate) fn guard_json_paths(value: &serde_json::Value) -> Result<(), String> {
    if !is_isolated() {
        return Ok(());
    }
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                let key = key.to_ascii_lowercase();
                let path_key = key.ends_with("_path")
                    || key.ends_with("_file")
                    || key.ends_with("_dir")
                    || ["path", "cwd", "transcript_cwd", "directory"].contains(&key.as_str());
                if path_key {
                    if let Some(path) = value.as_str() {
                        guard_path(Path::new(path))?;
                    }
                }
                guard_json_paths(value)?;
            }
        }
        serde_json::Value::Array(array) => {
            for value in array {
                guard_json_paths(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn guard_serialized_paths(value: &impl serde::Serialize) -> Result<(), String> {
    if !is_isolated() {
        return Ok(());
    }
    guard_json_paths(&serde_json::to_value(value).map_err(|error| error.to_string())?)
}

pub(crate) fn guard_action(name: &str, input: &serde_json::Value) -> Result<(), String> {
    if !is_isolated() {
        return Ok(());
    }
    const READS: &[&str] = &[
        "session.list",
        "session.get",
        "session.get_info",
        "git.list_branches",
        "git.current_branch",
        "git.worktree_list",
        "pty.status",
        "pty.list",
        "pty.snapshot",
        "pty.resize",
        "coordination.get_log",
        "coordination.get_workers_state",
        "coordination.get_session_storage_path",
        "coordination.get_current_directory",
        "coordination.list_stored_sessions",
        "coordination.get_app_config",
        "coordination.get_session_plan",
    ];
    if !READS.contains(&name) {
        return Err(format!("Isolated instance: action {name} is disabled"));
    }
    guard_action_ids(input)?;
    guard_json_paths(input)
}

pub(crate) fn guard_id(id: &str) -> Result<(), String> {
    if !is_isolated() {
        return Ok(());
    }
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("Isolated instance: IDs must be flat ASCII names".into());
    }
    Ok(())
}

pub(crate) fn guard_stored_session(
    expected_id: &str,
    session: &crate::storage::PersistedSession,
) -> Result<(), String> {
    if !is_isolated() {
        return Ok(());
    }
    guard_id(expected_id)?;
    guard_id(&session.id)?;
    if session.id != expected_id {
        return Err("Isolated instance: stored session ID mismatch".into());
    }
    for agent in &session.agents {
        guard_id(&agent.id)?;
    }
    guard_serialized_paths(session)
}

fn guard_action_ids(value: &serde_json::Value) -> Result<(), String> {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if ["id", "session_id", "agent_id", "parent_id"].contains(&key.as_str()) {
                    if let Some(id) = value.as_str() {
                        guard_id(id)?;
                    }
                }
                guard_action_ids(value)?;
            }
        }
        serde_json::Value::Array(array) => {
            for value in array {
                guard_action_ids(value)?;
            }
        }
        _ => {}
    }
    Ok(())
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
            assert_eq!(
                resolved_path(&std::env::current_dir().unwrap()).unwrap(),
                resolved_path(&settings.root).unwrap()
            );
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
        for plugin in [
            "tauri_plugin_dialog",
            "tauri_plugin_opener",
            "tauri_plugin_shell",
            "tauri_plugin_clipboard_manager",
        ] {
            assert!(
                plugins.contains(plugin),
                "native side-effect plugin outside isolation guard: {plugin}"
            );
            assert_eq!(lib.matches(plugin).count(), 1);
        }
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

    #[test]
    fn instance_fixture_overlay_states_without_ptys() {
        if std::env::var_os("HM_INSTANCE_OVERLAY_CHILD").is_some() {
            bootstrap();
            let settings = settings().unwrap();
            let base = crate::storage::SessionStorage::get_app_data_dir().unwrap();
            let storage =
                std::sync::Arc::new(crate::storage::SessionStorage::new_with_base(base).unwrap());
            let manager =
                std::sync::Arc::new(parking_lot::RwLock::new(crate::pty::PtyManager::new()));
            let mut controller = crate::session::SessionController::new(manager.clone());
            controller.set_storage(storage.clone());
            let path = settings.root.join("fixture-state.json");
            let valid = std::fs::read(&path).unwrap();
            let mut invalid: serde_json::Value = serde_json::from_slice(&valid).unwrap();
            invalid["version"] = 2.into();
            std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(load_fixtures(&controller, &storage).is_err());
            assert!(controller.list_sessions().is_empty());
            std::fs::write(&path, &valid).unwrap();
            let mut persisted = storage.load_session("synthetic").unwrap();
            let project = persisted.project_path.clone();
            persisted.project_path = settings
                .root
                .parent()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            storage.save_session(&persisted).unwrap();
            assert!(load_fixtures(&controller, &storage).is_err());
            assert!(controller.list_sessions().is_empty());
            persisted.project_path = project;
            storage.save_session(&persisted).unwrap();
            load_fixtures(&controller, &storage).unwrap();
            let session = controller.get_session("synthetic").unwrap();
            assert_eq!(session.agents[0].status, crate::pty::AgentStatus::Running);
            assert_eq!(
                session.agents[1].status,
                crate::pty::AgentStatus::Error("exited_during_startup:7".into())
            );
            assert!(
                matches!(&session.agents[2].status, crate::pty::AgentStatus::WaitingForInput(reason) if reason.starts_with("injected_delivery_failed:"))
            );
            assert_eq!(
                session.agents[3].work_state,
                Some(crate::session::work_state::WorkState::Finished)
            );
            assert_eq!(controller.get_heartbeat_info("synthetic").len(), 4);
            let stalled = controller
                .get_stalled_agent_reports("synthetic", std::time::Duration::from_secs(180));
            assert!(stalled.iter().any(|(id, _, _)| id == "synthetic-worker-1"));
            assert!(!stalled.iter().any(|(id, _, _)| id == "synthetic-worker-4"));
            controller.sweep_dead_startup_seats("synthetic");
            assert_eq!(
                controller.get_session("synthetic").unwrap().agents[3].work_state,
                Some(crate::session::work_state::WorkState::Finished)
            );
            assert!(manager.read().list_sessions().is_empty());
            return;
        }
        let (temp, env) = fixture();
        let Instance::Isolated(settings) = resolve(&env, BuildProfile::Debug).unwrap() else {
            panic!("fixture settings")
        };
        let storage = crate::storage::SessionStorage::new_with_base(settings.data_dir()).unwrap();
        let project = settings.root.join("projects/synthetic");
        std::fs::create_dir_all(&project).unwrap();
        let at = chrono::Utc::now() - chrono::Duration::minutes(5);
        let ids = (1..=4)
            .map(|n| format!("synthetic-worker-{n}"))
            .collect::<Vec<_>>();
        let agents = ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                serde_json::json!({
                    "id": id, "role": format!("Worker({})", index + 1), "parent_id": null,
                    "config": {"cli":"codex", "flags":[]}, "transcript_cwd":project,
                })
            })
            .collect::<Vec<_>>();
        let persisted: crate::storage::PersistedSession =
            serde_json::from_value(serde_json::json!({
                "id":"synthetic", "session_type":{"Hive":{"worker_count":4}},
                "project_path":project, "created_at":at, "agents":agents, "state":"Running"
            }))
            .unwrap();
        storage.save_session(&persisted).unwrap();
        let statuses = [
            serde_json::json!("Running"),
            serde_json::json!({"Error":"exited_during_startup:7"}),
            serde_json::json!({"WaitingForInput":format!("injected_delivery_failed:fixture:{}", at.to_rfc3339())}),
            serde_json::json!("Running"),
        ];
        let overlay = serde_json::json!({"version":1,"sessions":[{"session_id":"synthetic", "agents":
            ids.iter().enumerate().map(|(index,id)| serde_json::json!({
                "agent_id":id,"status":statuses[index],"work_state":if index==3 {"finished"} else {"working"},
                "heartbeat":{"status":if index==3 {"completed"} else {"working"},"summary":"Synthetic fixture","last_activity":at}
            })).collect::<Vec<_>>()
        }]});
        std::fs::write(
            temp.path().join("fixture-state.json"),
            serde_json::to_vec(&overlay).unwrap(),
        )
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance::tests::instance_fixture_overlay_states_without_ptys",
                "--nocapture",
            ])
            .envs(env)
            .env("HM_INSTANCE_OVERLAY_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // A normal process never even parses an overlay, including a malformed one.
        std::fs::write(temp.path().join("fixture-state.json"), "invalid").unwrap();
        let mut controller = crate::session::SessionController::new(std::sync::Arc::new(
            parking_lot::RwLock::new(crate::pty::PtyManager::new()),
        ));
        controller.set_storage(std::sync::Arc::new(storage));
        load_fixtures(
            &controller,
            &crate::storage::SessionStorage::new_with_base(settings.data_dir()).unwrap(),
        )
        .unwrap();
        assert!(controller.list_sessions().is_empty());
    }

    #[test]
    fn instance_refuses_default_profile_and_home() {
        let (temp, mut env) = fixture();
        let local = temp.path().join("real-localappdata");
        let profile = local.join("com.rduff.hive-manager");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join(".hive-instance"), "fixture").unwrap();
        env.insert("LOCALAPPDATA".into(), local.to_string_lossy().into_owned());
        env.insert(DIR.into(), profile.to_string_lossy().into_owned());
        assert!(resolve(&env, BuildProfile::Debug).is_err());
        env.insert(DIR.into(), temp.path().to_string_lossy().into_owned());
        assert!(
            resolve(&env, BuildProfile::Debug).is_err(),
            "profile ancestor"
        );
        env.remove("LOCALAPPDATA");
        env.insert(DIR.into(), profile.to_string_lossy().into_owned());
        env.insert("USERPROFILE".into(), profile.to_string_lossy().into_owned());
        assert!(resolve(&env, BuildProfile::Debug).is_err(), "real home");
        env.insert(
            "USERPROFILE".into(),
            temp.path().to_string_lossy().into_owned(),
        );
        assert!(
            matches!(
                resolve(&env, BuildProfile::Debug),
                Ok(Instance::Isolated(_))
            ),
            "separate folder under home is allowed"
        );
    }

    #[test]
    fn instance_environment_contract_is_complete() {
        let text = source("src/instance.rs");
        let environment = first_statement(&text, "environment")
            .split_once("    pub fn contains(")
            .unwrap()
            .0;
        let compact = environment.split_whitespace().collect::<String>();
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
            assert!(
                compact.contains(&format!("(\"{key}\",\"{relative}\")")),
                "redirect missing: {key}"
            );
        }
        for key in [
            "GIT_CONFIG_NOSYSTEM",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_SYSTEM",
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        ] {
            assert!(
                environment.contains(&format!("\"{key}\"")),
                "isolated override missing: {key}"
            );
        }
        let bootstrap = first_statement(&text, "bootstrap")
            .split_once("\npub fn refuse(")
            .unwrap()
            .0;
        assert!(bootstrap.contains("for (key, value) in settings.environment()"));
        assert!(bootstrap.contains("std::env::set_var(key, value)"));
        assert!(bootstrap.contains("std::env::remove_var(key)"));
        assert!(bootstrap.contains("std::env::set_current_dir(&settings.root)"));
        let scrub = first_statement(&text, "scrub_key")
            .split_once("\nfn reject_links(")
            .unwrap()
            .0;
        for prefix in ["HIVE_", "CLAUDE_CODE_", "GIT_", "WEBVIEW2_"] {
            assert!(
                scrub.contains(&format!("key.starts_with(\"{prefix}\")")),
                "inherited redirect/identity: {prefix}"
            );
        }
    }

    #[test]
    fn instance_no_pty_resize_guard() {
        let manager = source("src/pty/manager.rs");
        let resize = first_statement(&manager, "resize")
            .split_once("    pub fn instance_id(")
            .unwrap()
            .0;
        let compact = resize.split_whitespace().collect::<String>();
        assert!(compact.contains(
            "ifcrate::instance::is_isolated()&&!sessions.contains_key(id){returnOk(());}"
        ));
    }

    #[test]
    fn instance_fixture_seam_is_isolated_only() {
        let instance = source("src/instance.rs");
        assert!(first_statement(&instance, "load_fixtures")
            .starts_with("let Some(settings) = settings() else"));
        assert!(first_statement(&instance, "project_fixture_work_states")
            .starts_with("if !is_isolated()"));
        let controller = source("src/session/controller.rs");
        assert!(first_statement(&controller, "apply_instance_fixture")
            .starts_with("if !crate::instance::is_isolated()"));
        assert!(controller.contains("crate::instance::project_fixture_work_states(&mut session);"));
        assert!(source("src/lib.rs")
            .contains("instance::load_fixtures(&session_controller.read(), &storage)"));
    }

    #[test]
    fn instance_reparse_points_are_refused() {
        let (root, _) = fixture();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("escaping-directory");
        #[cfg(windows)]
        {
            let mut command = std::process::Command::new("cmd.exe");
            command
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&link)
                .arg(outside.path());
            crate::process::hide_std_console_window(&mut command);
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "junction setup: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let result = reject_links(root.path());
        let env = Environment::from([
            (DIR.into(), root.path().to_string_lossy().into_owned()),
            (PORT.into(), "18891".into()),
        ]);
        let Instance::Isolated(settings) = resolve(&env, BuildProfile::Debug).unwrap() else {
            panic!("fixture")
        };
        let contained = settings.contains(&link);
        #[cfg(windows)]
        std::fs::remove_dir(&link).unwrap();
        #[cfg(unix)]
        std::fs::remove_file(&link).unwrap();
        assert!(
            result.is_err(),
            "reparse point must be refused before opening storage"
        );
        assert!(
            contained.is_err(),
            "canonical target must remain in the instance"
        );
        assert!(outside.path().exists());
    }

    fn boundary_child(name: &str) -> bool {
        if std::env::var("HM_INSTANCE_BOUNDARY_TEST").ok().as_deref() == Some(name) {
            bootstrap();
            return true;
        }
        let (_root, env) = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            outside.path().join("sentinel.md"),
            "## Status: ACTIVE\nSynthetic outside fixture\n",
        )
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("instance::tests::{name}"),
                "--nocapture",
            ])
            .envs(env)
            .env("HM_INSTANCE_BOUNDARY_TEST", name)
            .env("HM_INSTANCE_OUTSIDE", outside.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        false
    }

    fn boundary_state() -> std::sync::Arc<crate::http::state::AppState> {
        use std::sync::Arc;
        let storage = Arc::new(
            crate::storage::SessionStorage::new_with_base(settings().unwrap().data_dir()).unwrap(),
        );
        let config = Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap()));
        let manager = Arc::new(parking_lot::RwLock::new(crate::pty::PtyManager::new()));
        let controller = Arc::new(parking_lot::RwLock::new(
            crate::session::SessionController::new(manager.clone()),
        ));
        controller.write().set_storage(storage.clone());
        let injection = Arc::new(parking_lot::RwLock::new(
            crate::coordination::InjectionManager::new(
                manager.clone(),
                crate::storage::SessionStorage::new_with_base(settings().unwrap().data_dir())
                    .unwrap(),
            ),
        ));
        let events = crate::events::EventBus::new(storage.base_dir().clone());
        let db = Arc::new(crate::storage::ApplicationStateDb::open_in_memory().unwrap());
        let repo = Arc::new(crate::storage::QueueRepo::new(db.clone()));
        repo.ensure_schema().unwrap();
        let queue = Arc::new(crate::coordination::QueueManager::new(repo, events.clone()));
        Arc::new(crate::http::state::AppState::new(
            config, manager, controller, injection, storage, events, db, queue, None,
        ))
    }

    struct BoundaryProbe(&'static str);
    #[async_trait::async_trait]
    impl crate::actions::Action for BoundaryProbe {
        fn name(&self) -> &'static str {
            self.0
        }
        fn input_schema(&self) -> schemars::schema::RootSchema {
            schemars::schema_for!(serde_json::Value)
        }
        async fn run(
            &self,
            _: &crate::actions::ActionContext,
            _: serde_json::Value,
        ) -> Result<serde_json::Value, crate::actions::ActionError> {
            Ok(serde_json::Value::Null)
        }
    }

    #[test]
    fn instance_registry_denies_mutations_and_external_paths() {
        let registry_source = source("src/actions/registry.rs");
        assert!(first_statement(&registry_source, "dispatch")
            .starts_with("crate::instance::guard_action("));
        if !boundary_child("instance_registry_denies_mutations_and_external_paths") {
            return;
        }
        let state = boundary_state();
        let mut registry = crate::actions::ActionRegistry::new();
        registry.register(Box::new(BoundaryProbe("git.worktree_add")));
        registry.register(Box::new(BoundaryProbe("git.current_branch")));
        let outside = std::env::var("HM_INSTANCE_OUTSIDE").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for caller in [
            crate::actions::Caller::Frontend,
            crate::actions::Caller::Http,
        ] {
            let context = crate::actions::ActionContext::new(caller, state.clone());
            let error = runtime.block_on(registry.dispatch("git.worktree_add", &context,
                serde_json::json!({"project_path":settings().unwrap().root,"worktree_path":settings().unwrap().root.join("projects/synthetic-worktree")}))).unwrap_err();
            assert!(error.message.contains("Isolated instance"));
            let error = runtime
                .block_on(registry.dispatch(
                    "git.current_branch",
                    &context,
                    serde_json::json!({"project_path":outside}),
                ))
                .unwrap_err();
            assert!(error.message.contains("Isolated instance"));
            assert!(runtime
                .block_on(registry.dispatch(
                    "git.current_branch",
                    &context,
                    serde_json::json!({"project_path":settings().unwrap().root})
                ))
                .is_ok());
        }
    }

    #[test]
    fn instance_git_paths_are_contained() {
        let git = source("src/actions/git/mod.rs");
        for name in ["run_git_in_dir", "run_git_ls_files_capped"] {
            let head = first_statement(&git, name)
                .split_once("if !path.exists()")
                .unwrap()
                .0;
            assert!(head.contains("crate::instance::guard_path(path)"), "{name}");
        }
        let controller = source("src/session/controller.rs");
        assert!(first_statement(&controller, "run_git_in_dir")
            .starts_with("crate::instance::guard_path(project_path)"));
        if !boundary_child("instance_git_paths_are_contained") {
            return;
        }
        let outside = std::env::var("HM_INSTANCE_OUTSIDE").unwrap();
        assert!(
            crate::actions::git::run_git_in_dir(&["--version"], &outside)
                .unwrap_err()
                .contains("Isolated instance")
        );
        assert!(crate::actions::git::run_git_ls_files_capped(&outside, 10, 1000).is_err());
        assert!(crate::actions::git::run_git_in_dir(
            &["--version"],
            settings().unwrap().root.to_str().unwrap()
        )
        .unwrap()
        .contains("git version"));
    }

    #[test]
    fn instance_tier_sources_are_contained() {
        let tier = source("src/cli/tier_ladder.rs");
        assert!(first_statement(&tier, "resolve_tier_ladder")
            .starts_with("if let Err(detail) = crate::instance::guard_path("));
        if !boundary_child("instance_tier_sources_are_contained") {
            return;
        }
        let outside = PathBuf::from(std::env::var("HM_INSTANCE_OUTSIDE").unwrap());
        let root = &settings().unwrap().root;
        for (project, wiki) in [(&outside, None), (root, Some(outside.as_path()))] {
            let result = crate::cli::tier_ladder::resolve_tier_ladder(project, wiki);
            assert!(result.ladder.is_none());
            assert!(result
                .issues
                .iter()
                .any(|issue| issue.detail.contains("Isolated instance")));
        }
        assert!(crate::cli::tier_ladder::resolve_tier_ladder(root, None)
            .ladder
            .is_some());
    }

    #[test]
    fn instance_review_paths_are_contained() {
        let review = source("src/review/diff.rs");
        for name in ["collect", "git", "load_untracked"] {
            assert!(
                first_statement(&review, name).starts_with("crate::instance::guard_path("),
                "{name}"
            );
        }
        assert!(review.contains("crate::instance::guard_path(&root.join(relative))?;"));
        if !boundary_child("instance_review_paths_are_contained") {
            return;
        }
        let outside = PathBuf::from(std::env::var("HM_INSTANCE_OUTSIDE").unwrap());
        let error = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(crate::review::diff::collect(
                &outside,
                Some("invalid"),
                "synthetic".into(),
            ))
            .unwrap_err();
        assert!(error.contains("Isolated instance"));
    }

    fn boundary_session(
        id: &str,
        project: &Path,
        kind: serde_json::Value,
    ) -> crate::storage::PersistedSession {
        serde_json::from_value(
            serde_json::json!({"id":id,"session_type":kind,"project_path":project,
            "created_at":chrono::Utc::now(),"agents":[],"state":"Running"}),
        )
        .unwrap()
    }

    #[test]
    fn instance_stored_sessions_are_contained_before_admission() {
        let storage_source = source("src/storage/mod.rs");
        assert!(first_statement(&storage_source, "load_session")
            .starts_with("crate::instance::guard_id("));
        assert!(
            storage_source.contains("crate::instance::guard_stored_session(session_id, &session)")
        );
        let load = first_statement(&storage_source, "load_session")
            .split_once("let json =")
            .unwrap()
            .0;
        assert!(load.contains("crate::instance::guard_path(&session_file)"));
        if !boundary_child("instance_stored_sessions_are_contained_before_admission") {
            return;
        }
        let state = boundary_state();
        let outside = PathBuf::from(std::env::var("HM_INSTANCE_OUTSIDE").unwrap());
        let session = boundary_session(
            "unlisted",
            &outside,
            serde_json::json!({"Hive":{"worker_count":0}}),
        );
        let directory = state.storage.create_session_dir("unlisted").unwrap();
        std::fs::write(
            directory.join("session.json"),
            serde_json::to_vec(&session).unwrap(),
        )
        .unwrap();
        assert!(state
            .storage
            .load_session("unlisted")
            .unwrap_err()
            .to_string()
            .contains("Isolated instance"));
        assert!(state
            .session_controller
            .read()
            .resume_session("unlisted")
            .unwrap_err()
            .contains("Isolated instance"));
        assert!(state.session_controller.read().list_sessions().is_empty());
    }

    #[test]
    fn instance_workspace_metadata_is_contained() {
        let controller_source = source("src/session/controller.rs");
        for kind in ["fusion", "debate"] {
            let method = first_statement(&controller_source, &format!("read_{kind}_metadata"));
            let before_read = method.split_once("let json =").unwrap().0;
            assert!(before_read.contains("crate::instance::guard_path(&metadata_path)"));
            let before_return = method.split_once("Ok(metadata)").unwrap().0;
            assert!(before_return.contains("crate::instance::guard_serialized_paths(&metadata)"));
        }
        if !boundary_child("instance_workspace_metadata_is_contained") {
            return;
        }
        let state = boundary_state();
        let root = &settings().unwrap().root;
        let outside = PathBuf::from(std::env::var("HM_INSTANCE_OUTSIDE").unwrap());
        let config = crate::pty::AgentConfig::default();
        for (id, kind, metadata) in [
            (
                "synthetic-fusion",
                serde_json::json!({"Fusion":{"variants":["synthetic"]}}),
                serde_json::json!({
                "base_branch":"main","variants":[{"index":1,"name":"Synthetic","slug":"synthetic","branch":"synthetic",
                    "worktree_path":root,"task_file":outside.join("sentinel.md"),"agent_id":"synthetic-agent"}],
                "judge_config":config,"task_description":"Synthetic","decision_file":outside.join("sentinel.md") }),
            ),
            (
                "synthetic-debate",
                serde_json::json!({"Debate":{"variants":["synthetic"]}}),
                serde_json::json!({
                "base_branch":"main","debaters":[{"index":1,"name":"Synthetic","stance":null,"slug":"synthetic",
                    "branch":"synthetic","worktree_path":outside,"config":config}],
                "judge_config":config,"topic":"Synthetic","rounds":1,"verdict_file":outside.join("sentinel.md") }),
            ),
        ] {
            state
                .storage
                .save_session(&boundary_session(id, root, kind))
                .unwrap();
            state.session_controller.read().resume_session(id).unwrap();
            let directory = root.join(".hive-manager").join(id);
            std::fs::create_dir_all(&directory).unwrap();
            let name = if id.ends_with("fusion") {
                "fusion-config.json"
            } else {
                "debate-config.json"
            };
            std::fs::write(directory.join(name), serde_json::to_vec(&metadata).unwrap()).unwrap();
        }
        let controller = state.session_controller.read();
        assert!(controller
            .get_fusion_variant_statuses("synthetic-fusion")
            .unwrap_err()
            .contains("Isolated instance"));
        assert!(controller
            .get_fusion_evaluation("synthetic-fusion")
            .unwrap_err()
            .contains("Isolated instance"));
        assert!(controller
            .get_debate_debater_statuses("synthetic-debate")
            .unwrap_err()
            .contains("Isolated instance"));
        assert!(controller
            .get_debate_evaluation("synthetic-debate")
            .unwrap_err()
            .contains("Isolated instance"));
    }

    #[test]
    fn instance_fusion_selection_is_disabled() {
        let controller = source("src/session/controller.rs");
        assert!(first_statement(&controller, "select_fusion_winner")
            .starts_with("crate::instance::refuse("));
        if !boundary_child("instance_fusion_selection_is_disabled") {
            return;
        }
        let error = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(crate::http::handlers::sessions::select_fusion_winner(
                axum::extract::State(boundary_state()),
                axum::extract::Path("synthetic".into()),
                axum::Json(crate::http::handlers::sessions::SelectFusionWinnerRequest {
                    variant: "synthetic".into(),
                }),
            ))
            .unwrap_err();
        assert!(error.message.contains("Isolated instance"));
    }

    #[test]
    fn instance_ids_and_project_admission_precede_io() {
        let handlers = source("src/http/handlers/mod.rs");
        assert!(first_statement(&handlers, "validate_session_id")
            .starts_with("crate::instance::guard_id("));
        assert!(first_statement(&handlers, "validate_project_path")
            .starts_with("crate::instance::guard_path("));
        assert!(first_statement(&handlers, "validate_candidate_ids")
            .split_once("if id.is_empty()")
            .unwrap()
            .0
            .contains("crate::instance::guard_id(id)"));
        assert!(first_statement(
            &source("src/commands/qa_commands.rs"),
            "validate_session_id"
        )
        .starts_with("crate::instance::guard_id("));
        let controller = source("src/session/controller.rs");
        assert!(first_statement(&controller, "resume_session")
            .starts_with("crate::instance::guard_id("));
        assert!(first_statement(&controller, "get_session")
            .starts_with("if crate::instance::guard_id(id).is_err()"));
        // This primitive performs no IO, so UNC refusal cannot contact a network share.
        assert!(absolute(Path::new(r"\\invalid-host\share\synthetic")).is_err());
        if !boundary_child("instance_ids_and_project_admission_precede_io") {
            return;
        }
        for id in ["D:entry", "../entry", "entry/child", "", "entry.child"] {
            assert!(guard_id(id).is_err());
            assert!(crate::http::handlers::validate_session_id(id).is_err());
            assert!(crate::http::handlers::validate_candidate_ids(&[id.into()]).is_err());
        }
        assert!(guard_id("synthetic-session_1").is_ok());
        let outside = std::env::var("HM_INSTANCE_OUTSIDE").unwrap();
        assert!(crate::http::handlers::validate_project_path(&outside)
            .unwrap_err()
            .message
            .contains("Isolated instance"));
    }

    #[test]
    fn instance_environment_roots_are_contained() {
        let (_temp, env) = fixture();
        let Instance::Isolated(mut settings) = resolve(&env, BuildProfile::Debug).unwrap() else {
            panic!("fixture")
        };
        settings.cdp_port = Some(18892);
        let environment = settings.environment();
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
            ("GIT_CONFIG_GLOBAL", "home/.gitconfig"),
            ("GIT_CONFIG_SYSTEM", "home/.gitconfig-system"),
        ] {
            assert_eq!(
                PathBuf::from(environment.get(key).unwrap()),
                settings.root.join(relative),
                "{key}"
            );
        }
        assert_eq!(environment.get("GIT_CONFIG_NOSYSTEM").unwrap(), "1");
        assert!(environment
            .get("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS")
            .unwrap()
            .contains("18892"));
        assert!(settings.contains(&settings.root.join("..")).is_err());
    }

    #[test]
    fn instance_network_paths_are_refused_without_io() {
        assert!(absolute(Path::new(r"\\invalid-host\share\synthetic")).is_err());
    }

    #[test]
    fn instance_bootstrap_checks_tree_before_writes() {
        let text = source("src/instance.rs");
        let bootstrap = first_statement(&text, "bootstrap")
            .split_once("\npub fn refuse(")
            .unwrap()
            .0;
        let links = bootstrap.find("reject_links(&settings.root)?;").unwrap();
        let writes = bootstrap.find("std::fs::create_dir_all").unwrap();
        assert!(links < writes);
    }

    #[test]
    fn instance_shared_spawn_policy_is_enforced() {
        if !boundary_child("instance_shared_spawn_policy_is_enforced") {
            assert!(refuse("synthetic operation").is_ok());
            return;
        }
        assert!(refuse("synthetic operation").is_err());
    }

    #[test]
    fn instance_shared_path_policy_is_enforced() {
        if !boundary_child("instance_shared_path_policy_is_enforced") {
            return;
        }
        let outside = PathBuf::from(std::env::var("HM_INSTANCE_OUTSIDE").unwrap());
        assert!(guard_path(&outside).is_err());
        assert!(guard_path(&settings().unwrap().root).is_ok());
    }
}
