//! Local Process Runtime - headless process execution via `std::process::Command`.
//!
//! This runtime provides simple process spawning for agents that don't require
//! terminal emulation (e.g., batch processing, CI/CD pipelines).

use std::collections::HashMap;
use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use super::{AgentProcessStatus, LaunchedAgent, LaunchSpec, RuntimeError, RuntimeAdapter};

/// A running process handle.
struct ProcessHandle {
    /// The child process.
    child: Mutex<Child>,
    /// Stdin for writing.
    stdin: Mutex<Option<ChildStdin>>,
}

/// Local headless process runtime adapter.
///
/// This adapter spawns agent processes without a PTY, suitable for
/// non-interactive or batch-style execution.
///
/// # Example
///
/// ```ignore
/// use hive_manager::runtime::{LocalProcessRuntime, LaunchSpec, RuntimeAdapter};
///
/// fn main() {
///     let runtime = LocalProcessRuntime::new();
///     let spec = LaunchSpec::new("echo")
///         .arg("Hello, World!");
///
///     let agent = runtime.launch(&spec)?;
///     // Wait for completion...
/// }
/// ```
pub struct LocalProcessRuntime {
    processes: Arc<Mutex<HashMap<String, ProcessHandle>>>,
}

impl Default for LocalProcessRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalProcessRuntime {
    /// Create a new LocalProcessRuntime.
    pub fn new() -> Self {
        Self {
            processes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Generate a unique process ID.
    fn generate_process_id() -> String {
        format!("proc-{}", uuid::Uuid::new_v4())
    }
}

impl RuntimeAdapter for LocalProcessRuntime {
    fn launch(&self, spec: &LaunchSpec) -> Result<LaunchedAgent, RuntimeError> {
        let process_id = Self::generate_process_id();

        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in &spec.env {
            cmd.env(key, value);
        }

        if let Some(ref cwd) = spec.cwd {
            cmd.current_dir(cwd);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| RuntimeError::launch(format!("Failed to spawn {}: {}", spec.command, e)))?;

        let stdin = child.stdin.take();

        let handle = ProcessHandle {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
        };

        let mut processes = self
            .processes
            .lock()
            .map_err(|_| RuntimeError::launch("Failed to lock processes"))?;
        processes.insert(process_id.clone(), handle);

        tracing::info!("Launched process: {} ({})", process_id, spec.command);

        Ok(LaunchedAgent {
            process_id,
            status: AgentProcessStatus::Starting,
        })
    }

    fn stop(&self, process_id: &str) -> Result<(), RuntimeError> {
        let handle = {
            let mut processes = self
                .processes
                .lock()
                .map_err(|_| RuntimeError::stop("Failed to lock processes"))?;
            processes
                .remove(process_id)
                .ok_or_else(|| RuntimeError::not_found(process_id))?
        };

        let mut child = handle
            .child
            .lock()
            .map_err(|_| RuntimeError::stop("Failed to lock child"))?;

        child
            .kill()
            .map_err(|e| RuntimeError::stop(format!("Kill failed: {}", e)))?;
        let _ = child.wait();

        tracing::info!("Stopped process: {}", process_id);

        Ok(())
    }

    fn write(&self, process_id: &str, input: &str) -> Result<(), RuntimeError> {
        let processes = self
            .processes
            .lock()
            .map_err(|_| RuntimeError::write("Failed to lock processes"))?;

        let handle = processes
            .get(process_id)
            .ok_or_else(|| RuntimeError::not_found(process_id))?;

        let mut stdin = handle
            .stdin
            .lock()
            .map_err(|_| RuntimeError::write("Failed to lock stdin"))?;

        if let Some(ref mut stdin) = *stdin {
            stdin
                .write_all(input.as_bytes())
                .map_err(|e| RuntimeError::write(format!("Write failed: {}", e)))?;
            stdin
                .flush()
                .map_err(|e| RuntimeError::write(format!("Flush failed: {}", e)))?;
        }

        Ok(())
    }

    fn resize(&self, _process_id: &str, _cols: u16, _rows: u16) -> Result<(), RuntimeError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeErrorKind;

    #[test]
    fn test_local_process_runtime_creation() {
        let runtime = LocalProcessRuntime::new();
        assert!(Arc::strong_count(&runtime.processes) >= 1);
    }

    #[test]
    fn test_process_id_generation() {
        let id1 = LocalProcessRuntime::generate_process_id();
        let id2 = LocalProcessRuntime::generate_process_id();

        assert!(id1.starts_with("proc-"));
        assert!(id2.starts_with("proc-"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_resize_is_noop() {
        let runtime = LocalProcessRuntime::new();
        let result = runtime.resize("nonexistent", 80, 24);
        assert!(result.is_ok());
    }

    #[test]
    fn test_launch_simple_command() {
        let runtime = LocalProcessRuntime::new();

        let spec = if cfg!(windows) {
            LaunchSpec::new("cmd").args(["/c", "echo", "test"])
        } else {
            LaunchSpec::new("echo").arg("test")
        };

        let result = runtime.launch(&spec);
        assert!(result.is_ok());

        let agent = result.unwrap();
        assert!(agent.process_id.starts_with("proc-"));
        assert_eq!(agent.status, AgentProcessStatus::Starting);
    }

    #[test]
    fn test_stop_nonexistent_process() {
        let runtime = LocalProcessRuntime::new();
        let result = runtime.stop("nonexistent");
        assert!(result.is_err());
        match result {
            Err(ref e) => {
                assert!(matches!(e.kind, RuntimeErrorKind::NotFound));
            }
            Ok(_) => panic!("Expected error"),
        }
    }

    #[test]
    fn test_write_nonexistent_process() {
        let runtime = LocalProcessRuntime::new();
        let result = runtime.write("nonexistent", "input");
        assert!(result.is_err());
        match result {
            Err(ref e) => {
                assert!(matches!(e.kind, RuntimeErrorKind::NotFound));
            }
            Ok(_) => panic!("Expected error"),
        }
    }
}
