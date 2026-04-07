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

unsafe impl Send for SendWriter {}
unsafe impl Sync for SendWriter {}

/// Wrapper to make the reader Send.
struct SendReader(Box<dyn io::Read + Send>);

unsafe impl Send for SendReader {}
unsafe impl Sync for SendReader {}

/// Wrapper to hold the master PTY handle.
struct MasterPtyHandle(Box<dyn portable_pty::MasterPty + Send>);

unsafe impl Send for MasterPtyHandle {}
unsafe impl Sync for MasterPtyHandle {}

/// A single PTY session for an agent.
struct PtySession {
    role: String,
    writer: Arc<Mutex<SendWriter>>,
    child: Arc<Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>>,
    master: Arc<Mutex<MasterPtyHandle>>,
}

impl PtySession {
    /// Create a new PTY session.
    fn new(
        role: &str,
        command: &str,
        args: &[String],
        cwd: Option<&PathBuf>,
        env: &HashMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> Result<Self, RuntimeError> {
        tracing::info!(
            "Creating PTY session: command={} args={:?} cwd={:?}",
            command,
            args,
            cwd
        );

        let pty_system = NativePtySystem::default();

        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| RuntimeError::launch(format!("Failed to open PTY: {}", e)))?;

        // On Windows, create a batch file to avoid shell quoting issues
        let mut cmd = if cfg!(windows) {
            let batch_content = Self::create_batch_content(command, args, env);
            let batch_path = Self::write_temp_batch(&batch_content)?;

            tracing::info!(
                "Created batch file: {} with content:\n{}",
                batch_path.display(),
                batch_content
            );

            let mut cmd = CommandBuilder::new("cmd.exe");
            cmd.args(&["/c", &batch_path.to_string_lossy()]);
            cmd
        } else {
            let mut cmd = CommandBuilder::new(command);
            cmd.args(args);
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
            role: role.to_string(),
            writer: Arc::new(Mutex::new(SendWriter(writer))),
            child: Arc::new(Mutex::new(Some(child))),
            master: Arc::new(Mutex::new(MasterPtyHandle(master))),
        })
    }

    /// Create batch file content for Windows.
    fn create_batch_content(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> String {
        let mut content = String::new();

        // Add environment variables
        for (key, value) in env {
            content.push_str(&format!("set {}={}\n", key, value));
        }

        // Build the command
        content.push_str(command);
        for arg in args {
            // Quote arguments that contain spaces
            if arg.contains(' ') || arg.contains('"') {
                content.push_str(&format!(" \"{}\"", arg.replace('"', "\"\"\"")));
            } else {
                content.push_str(&format!(" {}", arg));
            }
        }
        content.push('\n');

        content
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

    /// Check if the child process is still running.
    fn is_running(&self) -> bool {
        let mut child = match self.child.lock() {
            Ok(c) => c,
            Err(_) => return false,
        };

        if let Some(ref mut child) = *child {
            match child.try_wait() {
                Ok(None) => true, // Still running
                Ok(Some(_)) => false, // Exited
                Err(_) => false,
            }
        } else {
            false
        }
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
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| RuntimeError::stop("Failed to lock sessions"))?;

        let session = sessions
            .get(process_id)
            .ok_or_else(|| RuntimeError::not_found(process_id))?;

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

        assert!(content.contains("set API_KEY=secret123"));
        assert!(content.contains("claude"));
        assert!(content.contains("--model"));
        assert!(content.contains("opus"));
    }

    #[test]
    fn test_process_id_generation() {
        let id1 = LocalPtyRuntime::generate_process_id();
        let id2 = LocalPtyRuntime::generate_process_id();

        assert!(id1.starts_with("pty-"));
        assert!(id2.starts_with("pty-"));
        assert_ne!(id1, id2);
    }
}
