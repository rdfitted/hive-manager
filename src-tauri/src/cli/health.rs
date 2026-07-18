use axum::Json;
use futures::future::join_all;
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use crate::adapters::VALID_CLIS;

const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const CURSOR_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_WSL_DISTRO: &str = "Ubuntu";
const DEFAULT_WSL_BINARY_PATH: &str = "/root/.local/bin/agent";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoginStatus {
    Yes,
    No,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliHealth {
    pub cli: String,
    pub resolved: bool,
    pub bin_path: Option<String>,
    pub logged_in: LoginStatus,
    pub detail: String,
    pub stale_hint: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CliHealthResponse {
    pub clis: Vec<CliHealth>,
}

pub struct CliHealthRegistry;

impl CliHealthRegistry {
    pub async fn check_all() -> CliHealthResponse {
        let refreshed_path = refreshed_windows_path();
        let checks = VALID_CLIS
            .iter()
            .copied()
            .map(|cli| Self::check_cli(cli, refreshed_path.as_deref()));
        CliHealthResponse {
            clis: join_all(checks).await,
        }
    }

    async fn check_cli(cli: &str, refreshed_path: Option<&OsStr>) -> CliHealth {
        let binary = executable_for_cli(cli);
        let binary_label = if cli == "cursor" { "WSL" } else { binary };
        let Some(bin_path) = resolve_executable(binary) else {
            let stale_path = resolve_from_refreshed_path(binary, refreshed_path);
            let stale_hint = stale_path.is_some();
            return CliHealth {
                cli: cli.to_string(),
                resolved: false,
                bin_path: stale_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                logged_in: LoginStatus::Unknown,
                detail: if stale_hint {
                    format!(
                        "{binary_label} is on the updated system PATH; restart Hive Manager to use it"
                    )
                } else {
                    format!("{binary_label} was not found on the current PATH")
                },
                stale_hint,
            };
        };

        if cli == "cursor" {
            return check_cursor_health(cli, bin_path).await;
        }

        let (logged_in, detail) = match cli {
            "codex" => probe_login(&bin_path, &["login", "status"], "Codex").await,
            "antigravity" => (
                LoginStatus::Unknown,
                "agy is available; Antigravity authentication is managed out of band".to_string(),
            ),
            _ => (
                LoginStatus::Unknown,
                format!(
                    "{binary} is available; this CLI does not expose a supported login-status probe"
                ),
            ),
        };

        CliHealth {
            cli: cli.to_string(),
            resolved: true,
            bin_path: Some(bin_path.to_string_lossy().into_owned()),
            logged_in,
            detail,
            stale_hint: false,
        }
    }
}

#[tauri::command]
pub async fn get_cli_health() -> CliHealthResponse {
    CliHealthRegistry::check_all().await
}

pub async fn get_cli_health_http() -> Json<CliHealthResponse> {
    Json(CliHealthRegistry::check_all().await)
}

fn executable_for_cli(cli: &str) -> &str {
    match cli {
        "antigravity" => "agy",
        "cursor" => "wsl",
        _ => cli,
    }
}

async fn check_cursor_health(cli: &str, wsl_path: PathBuf) -> CliHealth {
    let distro = std::env::var("HIVE_WSL_DISTRO")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_WSL_DISTRO.to_string());
    let binary_path = std::env::var("HIVE_WSL_BINARY_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_WSL_BINARY_PATH.to_string());
    let args = [
        "-d".to_string(),
        distro.clone(),
        "--".to_string(),
        "test".to_string(),
        "-x".to_string(),
        binary_path.clone(),
    ];

    let (resolved, detail) = match run_bounded_command(&wsl_path, &args, CURSOR_PROBE_TIMEOUT).await
    {
        ProbeResult::Success(_) => (
            true,
            format!("WSL and Cursor agent {binary_path} are available in {distro}"),
        ),
        ProbeResult::Failure(output) => (
            false,
            if output.is_empty() {
                format!("WSL is available, but Cursor agent {binary_path} is missing in {distro}")
            } else {
                format!("Cursor agent probe failed in {distro}: {output}")
            },
        ),
        ProbeResult::TimedOut => (
            false,
            format!("WSL is available, but the Cursor agent probe timed out in {distro}"),
        ),
        ProbeResult::LaunchError(error) => (
            false,
            format!("WSL was found but could not probe the Cursor agent: {error}"),
        ),
    };

    CliHealth {
        cli: cli.to_string(),
        resolved,
        bin_path: Some(wsl_path.to_string_lossy().into_owned()),
        logged_in: LoginStatus::Unknown,
        detail,
        stale_hint: false,
    }
}

async fn probe_login(program: &Path, args: &[&str], label: &str) -> (LoginStatus, String) {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    match run_bounded_command(program, &args, AUTH_PROBE_TIMEOUT).await {
        ProbeResult::Success(output) => (
            LoginStatus::Yes,
            if output.is_empty() {
                format!("{label} reports an active login")
            } else {
                output
            },
        ),
        ProbeResult::Failure(output) => (
            LoginStatus::No,
            if output.is_empty() {
                format!("{label} reports that no login is active")
            } else {
                output
            },
        ),
        ProbeResult::TimedOut => (
            LoginStatus::Unknown,
            format!("{label} login-status probe timed out"),
        ),
        ProbeResult::LaunchError(error) => (
            LoginStatus::Unknown,
            format!("{label} login-status probe could not run: {error}"),
        ),
    }
}

enum ProbeResult {
    Success(String),
    Failure(String),
    TimedOut,
    LaunchError(String),
}

async fn run_bounded_command(program: &Path, args: &[String], timeout: Duration) -> ProbeResult {
    let (mut command, _temp_script) = match build_probe_command(program, args) {
        Ok(command) => command,
        Err(error) => return ProbeResult::LaunchError(error),
    };
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => {
            let detail = sanitize_probe_output(&output.stdout, &output.stderr);
            if output.status.success() {
                ProbeResult::Success(detail)
            } else {
                ProbeResult::Failure(detail)
            }
        }
        Ok(Err(error)) => ProbeResult::LaunchError(error.to_string()),
        Err(_) => ProbeResult::TimedOut,
    }
}

#[cfg(windows)]
fn build_probe_command(
    program: &Path,
    args: &[String],
) -> Result<(Command, Option<tempfile::TempPath>), String> {
    let is_batch = program
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
        });

    if !is_batch {
        let mut command = Command::new(program);
        command.args(args);
        return Ok((command, None));
    }

    use std::io::Write;
    let mut script = tempfile::Builder::new()
        .prefix("hive-cli-health-")
        .suffix(".bat")
        .tempfile()
        .map_err(|error| error.to_string())?;
    let mut line = format!("@call {}", quote_batch_argument(&program.to_string_lossy()));
    for arg in args {
        line.push(' ');
        line.push_str(&quote_batch_argument(arg));
    }
    writeln!(script, "@echo off\r\n{line}").map_err(|error| error.to_string())?;

    let temp_path = script.into_temp_path();
    let script_path = temp_path.to_path_buf();

    let mut command = Command::new("cmd.exe");
    command.arg("/d").arg("/c").arg(script_path);
    Ok((command, Some(temp_path)))
}

#[cfg(not(windows))]
fn build_probe_command(
    program: &Path,
    args: &[String],
) -> Result<(Command, Option<tempfile::TempPath>), String> {
    let mut command = Command::new(program);
    command.args(args);
    Ok((command, None))
}

#[cfg(windows)]
fn quote_batch_argument(value: &str) -> String {
    let escaped = value.replace('^', "^^").replace('%', "%%");
    format!("\"{escaped}\"")
}

fn sanitize_probe_output(stdout: &[u8], stderr: &[u8]) -> String {
    let bytes = if stdout.is_empty() { stderr } else { stdout };
    let normalized = String::from_utf8_lossy(bytes)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    normalized.chars().take(240).collect()
}

fn resolve_executable(executable: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let pathext = std::env::var_os("PATHEXT");
    resolve_executable_in_path(executable, &path, pathext.as_deref())
}

fn resolve_executable_in_path(
    executable: &str,
    path_value: &OsStr,
    pathext: Option<&OsStr>,
) -> Option<PathBuf> {
    let executable_path = Path::new(executable);
    if executable_path.components().count() > 1 {
        return executable_candidates(executable_path, pathext)
            .into_iter()
            .find(|candidate| is_executable_file(candidate));
    }

    std::env::split_paths(path_value)
        .filter(|directory| !directory.as_os_str().is_empty())
        .flat_map(|directory| {
            let directory = PathBuf::from(directory.to_string_lossy().trim_matches('"'));
            executable_candidates(&directory.join(executable), pathext)
        })
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(windows)]
fn executable_candidates(base: &Path, pathext: Option<&OsStr>) -> Vec<PathBuf> {
    if let Some(extension) = base.extension().and_then(OsStr::to_str) {
        let allowed = cmd_executable_extensions(pathext);
        let normalized = format!(".{}", extension.to_ascii_uppercase());
        return allowed.contains(&normalized).then(|| base.to_path_buf()).into_iter().collect();
    }

    cmd_executable_extensions(pathext)
        .into_iter()
        .map(|extension| PathBuf::from(format!("{}{extension}", base.to_string_lossy())))
        .collect()
}

#[cfg(not(windows))]
fn executable_candidates(base: &Path, _pathext: Option<&OsStr>) -> Vec<PathBuf> {
    vec![base.to_path_buf()]
}

#[cfg(windows)]
fn cmd_executable_extensions(pathext: Option<&OsStr>) -> Vec<String> {
    const CMD_EXTENSIONS: &[&str] = &[".COM", ".EXE", ".BAT", ".CMD"];
    let raw = pathext
        .and_then(OsStr::to_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(".COM;.EXE;.BAT;.CMD");
    raw
        .split(';')
        .map(|extension| extension.trim().to_ascii_uppercase())
        .filter(|extension| CMD_EXTENSIONS.contains(&extension.as_str()))
        .collect()
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn resolve_from_refreshed_path(
    executable: &str,
    refreshed_path: Option<&OsStr>,
) -> Option<PathBuf> {
    let refreshed_path = refreshed_path?;
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    if refreshed_path == current_path {
        return None;
    }
    let pathext = std::env::var_os("PATHEXT");
    resolve_executable_in_path(executable, &refreshed_path, pathext.as_deref())
}

#[cfg(windows)]
fn refreshed_windows_path() -> Option<OsString> {
    let mut values = Vec::new();
    for key in [
        r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        r"HKCU\Environment",
    ] {
        let Ok(output) = std::process::Command::new("reg.exe")
            .args(["query", key, "/v", "Path"])
            .output()
        else {
            continue;
        };
        if output.status.success() {
            if let Some(path) = parse_registry_path(&String::from_utf8_lossy(&output.stdout)) {
                values.push(expand_windows_env_vars(path));
            }
        }
    }

    (!values.is_empty()).then(|| OsString::from(values.join(";")))
}

#[cfg(not(windows))]
fn refreshed_windows_path() -> Option<OsString> {
    None
}

#[cfg(windows)]
fn parse_registry_path(output: &str) -> Option<&str> {
    output.lines().find_map(|line| {
        for marker in ["REG_EXPAND_SZ", "REG_SZ"] {
            if let Some(index) = line.find(marker) {
                if line[..index].trim().eq_ignore_ascii_case("Path") {
                    return Some(line[index + marker.len()..].trim());
                }
            }
        }
        None
    })
}

#[cfg(windows)]
fn expand_windows_env_vars(value: &str) -> String {
    let mut expanded = String::new();
    let mut remainder = value;
    while let Some(start) = remainder.find('%') {
        expanded.push_str(&remainder[..start]);
        let after_start = &remainder[start + 1..];
        let Some(end) = after_start.find('%') else {
            expanded.push_str(&remainder[start..]);
            return expanded;
        };
        let variable = &after_start[..end];
        match std::env::var(variable) {
            Ok(replacement) => expanded.push_str(&replacement),
            Err(_) => {
                expanded.push('%');
                expanded.push_str(variable);
                expanded.push('%');
            }
        }
        remainder = &after_start[end + 1..];
    }
    expanded.push_str(remainder);
    expanded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remapped_clis_use_the_real_launch_executable() {
        assert_eq!(executable_for_cli("antigravity"), "agy");
        assert_eq!(executable_for_cli("cursor"), "wsl");
        assert_eq!(executable_for_cli("codex"), "codex");
    }

    #[cfg(windows)]
    #[test]
    fn windows_resolution_accepts_cmd_shims_but_not_powershell_only_shims() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("codex.cmd"), "@echo off\r\n").unwrap();
        std::fs::write(directory.path().join("qwen.ps1"), "exit 0\r\n").unwrap();
        let path = directory.path().as_os_str();
        let pathext = OsStr::new(".COM;.EXE;.BAT;.CMD;.PS1");

        let codex = resolve_executable_in_path("codex", path, Some(pathext)).unwrap();
        assert_eq!(codex.file_name().unwrap().to_string_lossy(), "codex.CMD");
        assert!(resolve_executable_in_path("qwen", path, Some(pathext)).is_none());
    }
}
