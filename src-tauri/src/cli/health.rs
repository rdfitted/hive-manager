use axum::Json;
use futures::future::join_all;
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::adapters::VALID_CLIS;

const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const CURSOR_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_OUTPUT_CAP: usize = 16 * 1024;
const PROBE_DETAIL_MAX_CHARS: usize = 240;
#[cfg(windows)]
const REGISTRY_QUERY_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(windows)]
const REGISTRY_OUTPUT_CAP: usize = 64 * 1024;
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
        let refreshed_path = refreshed_windows_path().await;
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
            return unresolved_cli_health(cli, binary_label, stale_path);
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

        resolved_cli_health(cli, bin_path, logged_in, detail)
    }
}

fn unresolved_cli_health(cli: &str, binary_label: &str, stale_path: Option<PathBuf>) -> CliHealth {
    let stale_hint = stale_path.is_some();
    CliHealth {
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
    }
}

fn resolved_cli_health(
    cli: &str,
    bin_path: PathBuf,
    logged_in: LoginStatus,
    detail: String,
) -> CliHealth {
    CliHealth {
        cli: cli.to_string(),
        resolved: true,
        bin_path: Some(bin_path.to_string_lossy().into_owned()),
        logged_in,
        detail,
        stale_hint: false,
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

    let probe = run_bounded_command(&wsl_path, &args, CURSOR_PROBE_TIMEOUT).await;
    cursor_health_from_probe(cli, wsl_path, &distro, &binary_path, probe)
}

fn cursor_health_from_probe(
    cli: &str,
    wsl_path: PathBuf,
    distro: &str,
    binary_path: &str,
    probe: BoundedProcessResult,
) -> CliHealth {
    let (resolved, detail) = match probe {
        BoundedProcessResult::Exited { status, .. } if status.success() => (
            true,
            format!("WSL and Cursor agent {binary_path} are available in {distro}"),
        ),
        BoundedProcessResult::Exited { stdout, stderr, .. } => {
            let output = sanitize_probe_output(&stdout.bytes, &stderr.bytes);
            (
                false,
                if output.is_empty() {
                    format!(
                        "WSL is available, but Cursor agent {binary_path} is missing in {distro}"
                    )
                } else {
                    bounded_probe_detail(format!(
                        "Cursor agent probe failed in {distro}: {output}"
                    ))
                },
            )
        }
        BoundedProcessResult::TimedOut => (
            false,
            format!("WSL is available, but the Cursor agent probe timed out in {distro}"),
        ),
        BoundedProcessResult::RunError(error) => (
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
    let probe = run_bounded_command(program, &args, AUTH_PROBE_TIMEOUT).await;
    classify_login_probe(probe, label)
}

fn classify_login_probe(probe: BoundedProcessResult, label: &str) -> (LoginStatus, String) {
    match probe {
        BoundedProcessResult::Exited {
            status,
            stdout,
            stderr,
        } => classify_completed_login_probe(
            status.success(),
            status.code(),
            &stdout,
            &stderr,
            label,
        ),
        BoundedProcessResult::TimedOut => (
            LoginStatus::Unknown,
            format!("{label} login-status probe timed out"),
        ),
        BoundedProcessResult::RunError(error) => (
            LoginStatus::Unknown,
            format!("{label} login-status probe could not run: {error}"),
        ),
    }
}

fn classify_completed_login_probe(
    success: bool,
    exit_code: Option<i32>,
    stdout: &CappedBytes,
    stderr: &CappedBytes,
    label: &str,
) -> (LoginStatus, String) {
    let output = sanitize_probe_output(&stdout.bytes, &stderr.bytes);
    if success {
        return (
            LoginStatus::Yes,
            if output.is_empty() {
                format!("{label} reports an active login")
            } else {
                output
            },
        );
    }

    if label == "Codex"
        && exit_code == Some(1)
        && !stdout.truncated
        && !stderr.truncated
        && normalize_probe_output(&stdout.bytes).is_empty()
        && normalize_probe_output(&stderr.bytes) == "Not logged in"
    {
        return (LoginStatus::No, "Not logged in".to_string());
    }

    let exit_label = exit_code
        .map(|code| format!("code {code}"))
        .unwrap_or_else(|| "an unknown status".to_string());
    let detail = if output.is_empty() {
        format!("{label} login-status probe exited with {exit_label}")
    } else {
        format!("{label} login-status probe exited with {exit_label}: {output}")
    };
    (LoginStatus::Unknown, bounded_probe_detail(detail))
}

#[derive(Debug, Default)]
struct CappedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(Debug)]
enum BoundedProcessResult {
    Exited {
        status: ExitStatus,
        stdout: CappedBytes,
        stderr: CappedBytes,
    },
    TimedOut,
    RunError(String),
}

async fn run_bounded_command(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> BoundedProcessResult {
    let (command, _temp_script) = match build_probe_command(program, args) {
        Ok(command) => command,
        Err(error) => return BoundedProcessResult::RunError(error),
    };
    capture_bounded(command, timeout, PROBE_OUTPUT_CAP).await
}

async fn capture_bounded(
    mut command: Command,
    timeout: Duration,
    per_pipe_cap: usize,
) -> BoundedProcessResult {
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return BoundedProcessResult::RunError(error.to_string()),
    };
    let Some(stdout) = child.stdout.take() else {
        return BoundedProcessResult::RunError("failed to capture child stdout".to_string());
    };
    let Some(stderr) = child.stderr.take() else {
        return BoundedProcessResult::RunError("failed to capture child stderr".to_string());
    };

    let mut stdout_reader = tokio::spawn(read_capped(stdout, per_pipe_cap));
    let mut stderr_reader = tokio::spawn(read_capped(stderr, per_pipe_cap));
    let completed = tokio::time::timeout(timeout, async {
        let status = child.wait().await.map_err(|error| error.to_string())?;
        let stdout = (&mut stdout_reader)
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        let stderr = (&mut stderr_reader)
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok::<_, String>((status, stdout, stderr))
    })
    .await;

    match completed {
        Ok(Ok((status, stdout, stderr))) => BoundedProcessResult::Exited {
            status,
            stdout,
            stderr,
        },
        Ok(Err(error)) => {
            terminate_and_reap(&mut child).await;
            abort_readers(&mut stdout_reader, &mut stderr_reader).await;
            BoundedProcessResult::RunError(error)
        }
        Err(_) => {
            terminate_and_reap(&mut child).await;
            abort_readers(&mut stdout_reader, &mut stderr_reader).await;
            BoundedProcessResult::TimedOut
        }
    }
}

async fn read_capped<R>(mut reader: R, cap: usize) -> std::io::Result<CappedBytes>
where
    R: AsyncRead + Unpin,
{
    let mut captured = CappedBytes::default();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(captured);
        }

        let retained = read.min(cap.saturating_sub(captured.bytes.len()));
        captured.bytes.extend_from_slice(&buffer[..retained]);
        captured.truncated |= retained < read;
    }
}

async fn terminate_and_reap(child: &mut tokio::process::Child) {
    if let Err(error) = child.kill().await {
        tracing::debug!(error = %error, "Failed to kill bounded child process after deadline");
    }
    if let Err(error) = child.wait().await {
        tracing::debug!(error = %error, "Failed to reap bounded child process");
    }
}

async fn abort_readers(
    stdout_reader: &mut tokio::task::JoinHandle<std::io::Result<CappedBytes>>,
    stderr_reader: &mut tokio::task::JoinHandle<std::io::Result<CappedBytes>>,
) {
    stdout_reader.abort();
    stderr_reader.abort();
    let _ = stdout_reader.await;
    let _ = stderr_reader.await;
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
    let stdout = normalize_probe_output(stdout);
    let normalized = if stdout.is_empty() {
        normalize_probe_output(stderr)
    } else {
        stdout
    };
    bounded_probe_detail(normalized)
}

fn normalize_probe_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn bounded_probe_detail(detail: String) -> String {
    detail.chars().take(PROBE_DETAIL_MAX_CHARS).collect()
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
            let directory = trim_windows_path_quotes(directory);
            executable_candidates(&directory.join(executable), pathext)
        })
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(windows)]
fn trim_windows_path_quotes(path: PathBuf) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if units.len() >= 2
        && units.first() == Some(&(b'"' as u16))
        && units.last() == Some(&(b'"' as u16))
    {
        PathBuf::from(OsString::from_wide(&units[1..units.len() - 1]))
    } else {
        path
    }
}

#[cfg(not(windows))]
fn trim_windows_path_quotes(path: PathBuf) -> PathBuf {
    path
}

#[cfg(windows)]
fn executable_candidates(base: &Path, pathext: Option<&OsStr>) -> Vec<PathBuf> {
    if let Some(extension) = base.extension().and_then(OsStr::to_str) {
        let allowed = cmd_executable_extensions(pathext);
        let normalized = format!(".{}", extension.to_ascii_uppercase());
        return allowed
            .contains(&normalized)
            .then(|| base.to_path_buf())
            .into_iter()
            .collect();
    }

    cmd_executable_extensions(pathext)
        .into_iter()
        .map(|extension| {
            let mut candidate = base.as_os_str().to_os_string();
            candidate.push(extension);
            PathBuf::from(candidate)
        })
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
async fn refreshed_windows_path() -> Option<OsString> {
    refreshed_windows_path_with_command(
        |key| {
            let mut command = Command::new("reg.exe");
            command.args(["query", key, "/v", "Path"]);
            command
        },
        REGISTRY_QUERY_TIMEOUT,
    )
    .await
}

#[cfg(windows)]
async fn refreshed_windows_path_with_command<F>(
    mut build_query: F,
    query_timeout: Duration,
) -> Option<OsString>
where
    F: FnMut(&'static str) -> Command,
{
    let mut values = Vec::new();
    for key in [
        r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        r"HKCU\Environment",
    ] {
        match capture_bounded(build_query(key), query_timeout, REGISTRY_OUTPUT_CAP).await {
            BoundedProcessResult::Exited { status, stdout, .. }
                if status.success() && !stdout.truncated =>
            {
                if let Some(path) = parse_registry_path(&String::from_utf8_lossy(&stdout.bytes)) {
                    values.push(expand_windows_env_vars(path));
                }
            }
            BoundedProcessResult::Exited { .. } => {
                tracing::debug!("Skipping unsuccessful Windows PATH registry query");
            }
            BoundedProcessResult::TimedOut => {
                tracing::debug!("Windows PATH registry query timed out");
            }
            BoundedProcessResult::RunError(error) => {
                tracing::debug!(error = %error, "Windows PATH registry query failed to run");
            }
        }
    }

    (!values.is_empty()).then(|| OsString::from(values.join(";")))
}

#[cfg(not(windows))]
async fn refreshed_windows_path() -> Option<OsString> {
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

    const PROCESS_FIXTURE_MODE: &str = "HIVE_CLI_HEALTH_PROCESS_FIXTURE";
    const PROCESS_FIXTURE_READY: &str = "HIVE_CLI_HEALTH_PROCESS_READY";
    #[cfg(windows)]
    const PROCESS_FIXTURE_LOCK: &str = "HIVE_CLI_HEALTH_PROCESS_LOCK";
    const PROCESS_FIXTURE_TEST: &str = "cli::health::tests::bounded_process_fixture";

    fn captured(bytes: &[u8]) -> CappedBytes {
        CappedBytes {
            bytes: bytes.to_vec(),
            truncated: false,
        }
    }

    fn fixture_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("test executable path"));
        command
            .args([
                "--ignored",
                "--exact",
                PROCESS_FIXTURE_TEST,
                "--nocapture",
            ])
            .env(PROCESS_FIXTURE_MODE, mode);
        command
    }

    #[test]
    #[ignore]
    fn bounded_process_fixture() {
        use std::io::Write;

        let Ok(mode) = std::env::var(PROCESS_FIXTURE_MODE) else {
            return;
        };
        match mode.as_str() {
            "flood-failure" => {
                let stdout = std::thread::spawn(|| {
                    std::io::stdout()
                        .write_all(&vec![b'O'; 128 * 1024])
                        .expect("write fixture stdout");
                });
                let stderr = std::thread::spawn(|| {
                    std::io::stderr()
                        .write_all(&vec![b'E'; 128 * 1024])
                        .expect("write fixture stderr");
                });
                stdout.join().expect("stdout fixture thread");
                stderr.join().expect("stderr fixture thread");
                std::process::exit(7);
            }
            "registry-machine" => {
                println!(r"    Path    REG_EXPAND_SZ    C:\Machine\bin");
            }
            "registry-user" => {
                println!(r"    Path    REG_SZ    C:\User\bin");
            }
            "stall" => {
                #[cfg(windows)]
                let _lock = {
                    use std::os::windows::fs::OpenOptionsExt;

                    let lock_path = std::env::var_os(PROCESS_FIXTURE_LOCK)
                        .expect("stall fixture lock path");
                    std::fs::OpenOptions::new()
                        .create(true)
                        .truncate(true)
                        .write(true)
                        .share_mode(0)
                        .open(lock_path)
                        .expect("open exclusive fixture lock")
                };
                if let Some(ready_path) = std::env::var_os(PROCESS_FIXTURE_READY) {
                    std::fs::write(ready_path, b"ready").expect("write fixture readiness marker");
                }
                std::thread::sleep(Duration::from_secs(30));
            }
            other => panic!("unknown bounded-process fixture mode: {other}"),
        }
    }

    #[test]
    fn remapped_clis_use_the_real_launch_executable() {
        assert_eq!(executable_for_cli("antigravity"), "agy");
        assert_eq!(executable_for_cli("cursor"), "wsl");
        assert_eq!(executable_for_cli("codex"), "codex");
    }

    #[test]
    fn pure_health_outcomes_cover_unavailable_stale_and_auth_unknown() {
        let unavailable = unresolved_cli_health("codex", "codex", None);
        assert!(!unavailable.resolved);
        assert!(unavailable.bin_path.is_none());
        assert_eq!(unavailable.logged_in, LoginStatus::Unknown);
        assert!(!unavailable.stale_hint);

        let stale = unresolved_cli_health(
            "codex",
            "codex",
            Some(PathBuf::from(r"C:\updated\codex.cmd")),
        );
        assert!(!stale.resolved);
        assert!(stale.bin_path.is_some());
        assert!(stale.stale_hint);

        let auth_unknown = resolved_cli_health(
            "antigravity",
            PathBuf::from("agy"),
            LoginStatus::Unknown,
            "authentication is managed out of band".to_string(),
        );
        assert!(auth_unknown.resolved);
        assert_eq!(auth_unknown.logged_in, LoginStatus::Unknown);
        assert!(!auth_unknown.stale_hint);
    }

    #[test]
    fn codex_login_probe_classifies_only_the_documented_logged_out_response_as_no() {
        let empty = captured(b"");
        let logged_in = captured(b"Logged in using ChatGPT\n");
        let (status, _) =
            classify_completed_login_probe(true, Some(0), &empty, &logged_in, "Codex");
        assert_eq!(status, LoginStatus::Yes);

        let logged_out = captured(b"Not logged in\r\n");
        let (status, detail) =
            classify_completed_login_probe(false, Some(1), &empty, &logged_out, "Codex");
        assert_eq!(status, LoginStatus::No);
        assert_eq!(detail, "Not logged in");

        let configuration_error = captured(b"Error loading config");
        let (status, detail) = classify_completed_login_probe(
            false,
            Some(1),
            &empty,
            &configuration_error,
            "Codex",
        );
        assert_eq!(status, LoginStatus::Unknown);
        assert!(detail.contains("code 1"));
        assert!(detail.contains("Error loading config"));

        let truncated_logged_out = CappedBytes {
            bytes: b"Not logged in".to_vec(),
            truncated: true,
        };
        let (status, _) = classify_completed_login_probe(
            false,
            Some(1),
            &empty,
            &truncated_logged_out,
            "Codex",
        );
        assert_eq!(status, LoginStatus::Unknown);

        let (status, _) = classify_login_probe(BoundedProcessResult::TimedOut, "Codex");
        assert_eq!(status, LoginStatus::Unknown);
        let (status, detail) = classify_login_probe(
            BoundedProcessResult::RunError("program missing".to_string()),
            "Codex",
        );
        assert_eq!(status, LoginStatus::Unknown);
        assert!(detail.contains("program missing"));
    }

    #[cfg(any(unix, windows))]
    fn synthetic_exit_status(code: i32) -> ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            ExitStatusExt::from_raw(code << 8)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            ExitStatusExt::from_raw(code as u32)
        }
    }

    #[cfg(any(unix, windows))]
    fn exited_probe(code: i32, stdout: &[u8], stderr: &[u8]) -> BoundedProcessResult {
        BoundedProcessResult::Exited {
            status: synthetic_exit_status(code),
            stdout: captured(stdout),
            stderr: captured(stderr),
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn cursor_health_checks_both_wsl_and_the_in_distro_agent() {
        let available = cursor_health_from_probe(
            "cursor",
            PathBuf::from("wsl"),
            "Ubuntu",
            "/agent",
            exited_probe(0, b"", b""),
        );
        assert!(available.resolved);
        assert_eq!(available.logged_in, LoginStatus::Unknown);

        let missing = cursor_health_from_probe(
            "cursor",
            PathBuf::from("wsl"),
            "Ubuntu",
            "/agent",
            exited_probe(1, b"", b""),
        );
        assert!(!missing.resolved);
        assert!(missing.detail.contains("agent /agent is missing"));

        let timed_out = cursor_health_from_probe(
            "cursor",
            PathBuf::from("wsl"),
            "Ubuntu",
            "/agent",
            BoundedProcessResult::TimedOut,
        );
        assert!(!timed_out.resolved);
        assert!(timed_out.detail.contains("timed out"));
    }

    #[tokio::test]
    async fn bounded_process_caps_both_pipes_and_preserves_failure_status() {
        let result = capture_bounded(
            fixture_command("flood-failure"),
            Duration::from_secs(5),
            1024,
        )
        .await;
        let BoundedProcessResult::Exited {
            status,
            stdout,
            stderr,
        } = result
        else {
            panic!("flood fixture did not exit: {result:?}");
        };
        assert_eq!(status.code(), Some(7));
        assert_eq!(stdout.bytes.len(), 1024);
        assert_eq!(stderr.bytes.len(), 1024);
        assert!(stdout.truncated);
        assert!(stderr.truncated);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn refreshed_path_queries_are_bounded_async_killable_and_best_effort() {
        use std::time::Instant;

        let directory = tempfile::tempdir().unwrap();
        let ready_path = directory.path().join("ready");
        let lock_path = directory.path().join("child.lock");
        let query_ready_path = ready_path.clone();
        let query_lock_path = lock_path.clone();
        let refresh = tokio::spawn(async move {
            refreshed_windows_path_with_command(
                move |key| {
                    if key.starts_with("HKLM") {
                        let mut command = fixture_command("stall");
                        command
                            .env(PROCESS_FIXTURE_READY, &query_ready_path)
                            .env(PROCESS_FIXTURE_LOCK, &query_lock_path);
                        command
                    } else {
                        fixture_command("registry-user")
                    }
                },
                Duration::from_millis(500),
            )
            .await
        });

        for _ in 0..100 {
            if ready_path.is_file() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ready_path.is_file(), "the stalled child fixture never started");

        let timer_start = Instant::now();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(timer_start.elapsed() < Duration::from_millis(300));

        let refreshed = refresh.await.unwrap();

        assert_eq!(refreshed, Some(OsString::from(r"C:\User\bin")));
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("timed-out registry child should be killed and its lock released");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn refreshed_path_combines_successful_machine_and_user_queries() {
        let refreshed = refreshed_windows_path_with_command(
            |key| {
                if key.starts_with("HKLM") {
                    fixture_command("registry-machine")
                } else {
                    fixture_command("registry-user")
                }
            },
            Duration::from_secs(5),
        )
        .await;

        assert_eq!(
            refreshed,
            Some(OsString::from(r"C:\Machine\bin;C:\User\bin"))
        );
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

    #[cfg(windows)]
    #[test]
    fn windows_resolution_preserves_non_unicode_paths_and_appends_pathext_as_os_strings() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let quoted = [b'"' as u16, b'C' as u16, b':' as u16, 0xd800, b'"' as u16];
        let trimmed = trim_windows_path_quotes(PathBuf::from(OsString::from_wide(&quoted)));
        assert_eq!(
            trimmed.as_os_str().encode_wide().collect::<Vec<_>>(),
            quoted[1..quoted.len() - 1]
        );

        let unmatched = [b'"' as u16, b'C' as u16, b':' as u16, 0xd800];
        let unchanged = trim_windows_path_quotes(PathBuf::from(OsString::from_wide(&unmatched)));
        assert_eq!(
            unchanged.as_os_str().encode_wide().collect::<Vec<_>>(),
            unmatched
        );

        let raw_base = [b'C' as u16, b':' as u16, b'\\' as u16, 0xd800, b'x' as u16];
        let base = PathBuf::from(OsString::from_wide(&raw_base));
        let candidates = executable_candidates(
            &base,
            Some(OsStr::new(".PS1;.cmd;.BAT;.exe")),
        );
        let expected_suffixes = [".CMD", ".BAT", ".EXE"];
        assert_eq!(candidates.len(), expected_suffixes.len());
        for (candidate, suffix) in candidates.iter().zip(expected_suffixes) {
            let units = candidate.as_os_str().encode_wide().collect::<Vec<_>>();
            assert_eq!(&units[..raw_base.len()], &raw_base);
            assert_eq!(
                &units[raw_base.len()..],
                OsStr::new(suffix).encode_wide().collect::<Vec<_>>()
            );
        }

        assert_eq!(
            executable_candidates(Path::new("codex.cmd"), Some(OsStr::new(".CMD"))),
            vec![PathBuf::from("codex.cmd")]
        );
        assert!(executable_candidates(Path::new("qwen.ps1"), Some(OsStr::new(".PS1"))).is_empty());
    }
}
