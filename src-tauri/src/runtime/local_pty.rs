//! Local PTY Runtime - PTY-based process execution via portable_pty.
//!
//! This runtime provides terminal emulation for agent processes that require
//! interactive terminal sessions (e.g., CLI tools with TUI interfaces).

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use super::{AgentProcessStatus, LaunchedAgent, LaunchSpec, RuntimeError, RuntimeAdapter};

/// Maximum chunk size for PTY writes (to respect buffer limits).
const CHUNK_SIZE: usize = 16 * 1024;

/// Wrapper to make the writer Send.
struct SendWriter(Box<dyn io::Write + Send>);

/// Wrapper to hold the master PTY handle.
struct MasterPtyHandle(Box<dyn portable_pty::MasterPty + Send>);

/// A single PTY session for an agent.
struct PtySession {
    _role: String,
    writer: Arc<Mutex<SendWriter>>,
    child: Arc<Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>>,
    master: Arc<Mutex<MasterPtyHandle>>,
    temp_batch_path: Option<PathBuf>,
}

impl PtySession {
    fn escape_batch_value(value: &str) -> String {
        let mut escaped = String::with_capacity(value.len());
        for ch in value.chars() {
            match ch {
                '%' => escaped.push_str("%%"),
                '"' | '^' | '&' | '|' | '<' | '>' | '(' | ')' => {
                    escaped.push('^');
                    escaped.push(ch);
                }
                _ => escaped.push(ch),
            }
        }
        escaped
    }

    fn quote_batch_argument(value: &str) -> String {
        format!("\"{}\"", Self::escape_batch_value(value))
    }

    /// Create a new PTY session.
    fn new(
        role: &str,
        command: &str,
        args: &[String],
        cwd: Option<&PathBuf>,
        env: &HashMap<String, String>,
        cols: u16,
        rows: u16,
        wsl_distro: Option<&str>,
        wsl_binary_path: Option<&str>,
    ) -> Result<Self, RuntimeError> {
        tracing::info!("Creating PTY session: command={} cwd={:?}", command, cwd);
        tracing::debug!("PTY session args: {:?}", args);

        let pty_system = NativePtySystem::default();

        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| RuntimeError::launch(format!("Failed to open PTY: {}", e)))?;

        let resolved_args = Self::apply_wsl_overrides(command, args, wsl_distro, wsl_binary_path);

        let temp_batch_path = Self::resolve_temp_batch_path(command, &resolved_args, env)?;

        // On Windows, create a batch file to avoid shell quoting issues
        let mut cmd = if cfg!(windows) {
            let mut cmd = CommandBuilder::new("cmd.exe");
            let batch_path = temp_batch_path
                .as_ref()
                .ok_or_else(|| RuntimeError::launch("Failed to prepare Windows batch file"))?;
            cmd.args(&["/c", &batch_path.to_string_lossy()]);
            cmd
        } else {
            let mut cmd = CommandBuilder::new(command);
            cmd.args(&resolved_args);
            // Set environment variables
            for (key, value) in env {
                cmd.env(key, value);
            }
            cmd
        };

        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| RuntimeError::launch(format!("Failed to spawn command: {}", e)))?;

        let writer = pty_pair
            .master
            .take_writer()
            .map_err(|e| RuntimeError::launch(format!("Failed to take writer: {}", e)))?;

        // Keep the master alive - dropping it closes the PTY!
        let master = pty_pair.master;

        Ok(Self {
            _role: role.to_string(),
            writer: Arc::new(Mutex::new(SendWriter(writer))),
            child: Arc::new(Mutex::new(Some(child))),
            master: Arc::new(Mutex::new(MasterPtyHandle(master))),
            temp_batch_path,
        })
    }

    /// Apply configurable WSL overrides while preserving existing command shape.
    fn apply_wsl_overrides(
        command: &str,
        args: &[String],
        wsl_distro: Option<&str>,
        wsl_binary_path: Option<&str>,
    ) -> Vec<String> {
        if !command.eq_ignore_ascii_case("wsl") {
            return args.to_vec();
        }

        let mut resolved = args.to_vec();

        let distro_override = wsl_distro
            .map(str::to_string)
            .or_else(|| std::env::var("HIVE_WSL_DISTRO").ok());
        if let Some(distro) = distro_override {
            if let Some(index) = resolved.iter().position(|arg| arg == "-d") {
                if index + 1 < resolved.len() {
                    resolved[index + 1] = distro;
                } else {
                    resolved.push(distro);
                }
            } else {
                resolved.insert(0, distro);
                resolved.insert(0, "-d".to_string());
            }
        }

        let binary_override = wsl_binary_path
            .map(str::to_string)
            .or_else(|| std::env::var("HIVE_WSL_BINARY_PATH").ok());
        if let Some(binary_path) = binary_override {
            if let Some(index) = resolved
                .iter()
                .position(|arg| arg == "/root/.local/bin/agent")
            {
                resolved[index] = binary_path;
            } else if resolved.len() >= 3 && resolved.first().map(|arg| arg.as_str()) == Some("-d") {
                resolved[2] = binary_path;
            }
        }

        resolved
    }

    /// Create batch file content for Windows.
    fn create_batch_content(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> String {
        let mut lines = vec!["@echo off".to_string()];

        // Add environment variables
        for (key, value) in env {
            lines.push(format!(
                "set \"{}={}\"",
                key,
                Self::escape_batch_value(value)
            ));
        }

        // Build the command
        let mut command_line = Self::quote_batch_argument(command);
        for arg in args {
            command_line.push(' ');
            command_line.push_str(&Self::quote_batch_argument(arg));
        }
        lines.push(command_line);

        lines.join("\r\n")
    }

    /// Write a temporary batch file for Windows execution.
    fn write_temp_batch(content: &str) -> Result<PathBuf, RuntimeError> {
        use std::fs::File;
        use std::io::Write as IoWrite;

        let temp_dir = std::env::temp_dir();
        let batch_name = format!("hive_agent_{}.bat", uuid::Uuid::new_v4());
        let batch_path = temp_dir.join(batch_name);

        let mut file = File::create(&batch_path)
            .map_err(|e| RuntimeError::launch(format!("Failed to create batch file: {}", e)))?;

        file.write_all(content.as_bytes())
            .map_err(|e| RuntimeError::launch(format!("Failed to write batch file: {}", e)))?;

        Ok(batch_path)
    }

    fn resolve_temp_batch_path(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Option<PathBuf>, RuntimeError> {
        if !cfg!(windows) {
            return Ok(None);
        }

        let batch_content = Self::create_batch_content(command, args, env);
        let batch_path = Self::write_temp_batch(&batch_content)?;

        tracing::info!(
            "Created batch file: {} with content:\n{}",
            batch_path.display(),
            batch_content
        );

        Ok(Some(batch_path))
    }

    /// Write data to the PTY.
    fn write(&self, data: &[u8]) -> Result<(), RuntimeError> {
        tracing::debug!("PTY write: {} bytes", data.len());
        let mut writer = self.writer.lock().map_err(|_| RuntimeError::write("Failed to lock writer"))?;

        // Write in chunks to respect buffer limits
        for chunk in data.chunks(CHUNK_SIZE) {
            writer
                .0
                .write_all(chunk)
                .map_err(|e| RuntimeError::write(format!("Write failed: {}", e)))?;
            writer
                .0
                .flush()
                .map_err(|e| RuntimeError::write(format!("Flush failed: {}", e)))?;
        }

        Ok(())
    }

    /// Resize the PTY terminal.
    fn resize(&self, cols: u16, rows: u16) -> Result<(), RuntimeError> {
        let master = self
            .master
            .lock()
            .map_err(|_| RuntimeError::resize("Failed to lock master"))?;
        master
            .0
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| RuntimeError::resize(format!("Resize failed: {}", e)))?;
        tracing::debug!("PTY resized to {}x{}", cols, rows);
        Ok(())
    }

    /// Kill the child process.
    fn kill(&self) -> Result<(), RuntimeError> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| RuntimeError::stop("Failed to lock child"))?;

        if let Some(ref mut child) = *child {
            child
                .kill()
                .map_err(|e| RuntimeError::stop(format!("Kill failed: {}", e)))?;
        }

        Ok(())
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if let Some(batch_path) = &self.temp_batch_path {
            if let Err(error) = std::fs::remove_file(batch_path) {
                if error.kind() != io::ErrorKind::NotFound {
                    tracing::warn!(
                        "Failed to remove temp batch file {}: {}",
                        batch_path.display(),
                        error
                    );
                }
            }
        }
    }
}

/// Local PTY-based runtime adapter.
///
/// This adapter spawns agent processes with a PTY (pseudo-terminal),
/// providing full terminal emulation for interactive CLIs.
///
/// # Example
///
/// ```ignore
/// use hive_manager::runtime::{LocalPtyRuntime, LaunchSpec, RuntimeAdapter};
///
/// let runtime = LocalPtyRuntime::new();
/// let spec = LaunchSpec::new("claude")
///     .arg("--dangerously-skip-permissions")
///     .cwd("/project");
///
/// let agent = runtime.launch(&spec)?;
/// runtime.write(&agent.process_id, "Hello\n")?;
/// runtime.stop(&agent.process_id)?;
/// ```
pub struct LocalPtyRuntime {
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
}

impl Default for LocalPtyRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalPtyRuntime {
    /// Create a new LocalPtyRuntime.
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Generate a unique process ID.
    fn generate_process_id() -> String {
        format!("pty-{}", uuid::Uuid::new_v4())
    }
}

impl RuntimeAdapter for LocalPtyRuntime {
    fn launch(&self, spec: &LaunchSpec) -> Result<LaunchedAgent, RuntimeError> {
        let process_id = Self::generate_process_id();

        let session = PtySession::new(
            &spec.role,
            &spec.command,
            &spec.args,
            spec.cwd.as_ref(),
            &spec.env,
            spec.cols,
            spec.rows,
            spec.wsl_distro.as_deref(),
            spec.wsl_binary_path.as_deref(),
        )?;

        // Store session
        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| RuntimeError::launch("Failed to lock sessions"))?;
            sessions.insert(process_id.clone(), session);
        }

        tracing::info!("Launched PTY process: {} ({})", process_id, spec.command);

        Ok(LaunchedAgent {
            process_id,
            status: AgentProcessStatus::Starting,
        })
    }

    fn stop(&self, process_id: &str) -> Result<(), RuntimeError> {
        let session = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| RuntimeError::stop("Failed to lock sessions"))?;
            sessions
                .remove(process_id)
                .ok_or_else(|| RuntimeError::not_found(process_id))?
        };

        session.kill()?;

        tracing::info!("Stopped PTY process: {}", process_id);

        Ok(())
    }

    fn write(&self, process_id: &str, input: &str) -> Result<(), RuntimeError> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| RuntimeError::write("Failed to lock sessions"))?;

        let session = sessions
            .get(process_id)
            .ok_or_else(|| RuntimeError::not_found(process_id))?;

        session.write(input.as_bytes())?;

        Ok(())
    }

    fn resize(&self, process_id: &str, cols: u16, rows: u16) -> Result<(), RuntimeError> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| RuntimeError::resize("Failed to lock sessions"))?;

        let session = sessions
            .get(process_id)
            .ok_or_else(|| RuntimeError::not_found(process_id))?;

        session.resize(cols, rows)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_pty_runtime_creation() {
        let runtime = LocalPtyRuntime::new();
        // Just verify it can be created
        assert!(Arc::strong_count(&runtime.sessions) >= 1);
    }

    #[test]
    fn test_pty_session_batch_content() {
        let env: HashMap<String, String> = [
            ("API_KEY".to_string(), "secret123".to_string()),
        ]
        .into_iter()
        .collect();

        let content = PtySession::create_batch_content(
            "claude",
            &["--model".to_string(), "opus".to_string()],
            &env,
        );

        assert!(content.contains("@echo off"));
        assert!(content.contains("set \"API_KEY=secret123\""));
        assert!(content.contains("\"claude\""));
        assert!(content.contains("\"--model\""));
        assert!(content.contains("\"opus\""));
    }

    #[test]
    fn test_pty_session_batch_content_escapes_windows_metacharacters() {
        let env: HashMap<String, String> = [("API_KEY".to_string(), "se%cr&et^\"".to_string())]
            .into_iter()
            .collect();

        let content = PtySession::create_batch_content(
            "C:\\Program Files\\Agent\\agent.exe",
            &["hello & goodbye".to_string(), "%TEMP%".to_string()],
            &env,
        );

        assert!(content.contains("set \"API_KEY=se%%cr^&et^^^\"\""));
        assert!(content.contains("\"C:\\Program Files\\Agent\\agent.exe\""));
        assert!(content.contains("\"hello ^& goodbye\""));
        assert!(content.contains("\"%%TEMP%%\""));
    }

    #[test]
    fn test_process_id_generation() {
        let id1 = LocalPtyRuntime::generate_process_id();
        let id2 = LocalPtyRuntime::generate_process_id();

        assert!(id1.starts_with("pty-"));
        assert!(id2.starts_with("pty-"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_apply_wsl_overrides() {
        let args = vec![
            "-d".to_string(),
            "Ubuntu".to_string(),
            "/root/.local/bin/agent".to_string(),
            "--force".to_string(),
        ];

        let resolved = PtySession::apply_wsl_overrides(
            "wsl",
            &args,
            Some("Ubuntu-24.04"),
            Some("/opt/cursor-agent"),
        );

        assert_eq!(resolved[1], "Ubuntu-24.04");
        assert_eq!(resolved[2], "/opt/cursor-agent");
    }
}
