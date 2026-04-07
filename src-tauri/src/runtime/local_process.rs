//! Local Process Runtime - headless process execution via tokio::process::Command.
//!
//! This runtime provides simple process spawning for agents that don't require
//! terminal emulation (e.g., batch processing, CI/CD pipelines).

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::RwLock;

use super::{AgentProcessStatus, LaunchedAgent, LaunchSpec, RuntimeError, RuntimeAdapter};

/// A running process handle.
struct ProcessHandle {
    /// The child process
    child: RwLock<Option<tokio::process::Child>>,
    /// Stdin for writing
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
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
/// #[tokio::main]
/// async fn main() {
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

    /// Launch a process asynchronously (internal helper).
    async fn launch_async(&self, spec: &LaunchSpec) -> Result<LaunchedAgent, RuntimeError> {
        let process_id = Self::generate_process_id();

        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Set environment variables
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }

        // Set working directory
        if let Some(ref cwd) = spec.cwd {
            cmd.current_dir(cwd);
        }

        // Spawn the process
        let mut child = cmd
            .spawn()
            .map_err(|e| RuntimeError::launch(format!("Failed to spawn {}: {}", spec.command, e)))?;

        // Take stdin handle
        let stdin = child.stdin.take();

        // Store the process handle
        let handle = ProcessHandle {
            child: RwLock::new(Some(child)),
            stdin: Mutex::new(stdin),
        };

        {
            let mut processes = self
                .processes
                .lock()
                .map_err(|_| RuntimeError::launch("Failed to lock processes"))?;
            processes.insert(process_id.clone(), handle);
        }

        tracing::info!("Launched process: {} ({})", process_id, spec.command);

        Ok(LaunchedAgent {
            process_id,
            status: AgentProcessStatus::Starting,
        })
    }

    /// Write to a process asynchronously (internal helper).
    async fn write_async(&self, process_id: &str, input: &str) -> Result<(), RuntimeError> {
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
                .await
                .map_err(|e| RuntimeError::write(format!("Write failed: {}", e)))?;
            stdin
                .flush()
                .await
                .map_err(|e| RuntimeError::write(format!("Flush failed: {}", e)))?;
        }

        Ok(())
    }

    /// Stop a process asynchronously (internal helper).
    async fn stop_async(&self, process_id: &str) -> Result<(), RuntimeError> {
        let processes = self
            .processes
            .lock()
            .map_err(|_| RuntimeError::stop("Failed to lock processes"))?;

        let handle = processes
            .get(process_id)
            .ok_or_else(|| RuntimeError::not_found(process_id))?;

        let mut child_guard = handle
            .child
            .write()
            .await;

        if let Some(ref mut child) = *child_guard {
            child
                .kill()
                .await
                .map_err(|e| RuntimeError::stop(format!("Kill failed: {}", e)))?;
        }

        tracing::info!("Stopped process: {}", process_id);

        Ok(())
    }
}

impl RuntimeAdapter for LocalProcessRuntime {
    fn launch(&self, spec: &LaunchSpec) -> Result<LaunchedAgent, RuntimeError> {
        // Since RuntimeAdapter is synchronous, we use tokio::runtime::Handle
        // if available, otherwise block_on
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| RuntimeError::launch(format!("Failed to create runtime: {}", e)))?;

        rt.block_on(self.launch_async(spec))
    }

    fn stop(&self, process_id: &str) -> Result<(), RuntimeError> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| RuntimeError::stop(format!("Failed to create runtime: {}", e)))?;

        rt.block_on(self.stop_async(process_id))
    }

    fn write(&self, process_id: &str, input: &str) -> Result<(), RuntimeError> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| RuntimeError::write(format!("Failed to create runtime: {}", e)))?;

        rt.block_on(self.write_async(process_id, input))
    }

    fn resize(&self, _process_id: &str, _cols: u16, _rows: u16) -> Result<(), RuntimeError> {
        // Headless processes don't support terminal resize
        // Return Ok to allow callers to safely call this method
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
        // Just verify it can be created
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
        // Resize should succeed as a no-op
        let result = runtime.resize("nonexistent", 80, 24);
        assert!(result.is_ok());
    }

    #[test]
    fn test_launch_simple_command() {
        let runtime = LocalProcessRuntime::new();

        // On Windows, use cmd.exe; on Unix, use echo
        let spec = if cfg!(windows) {
            LaunchSpec::new("cmd")
                .args(["/c", "echo", "test"])
        } else {
            LaunchSpec::new("echo")
                .arg("test")
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
