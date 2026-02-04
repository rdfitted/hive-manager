use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::sync::Arc;
use parking_lot::Mutex;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentRole {
    Queen,
    Planner { index: u8 },
    Worker { index: u8, parent: Option<String> },
    Fusion { variant: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus {
    Starting,
    Running,
    WaitingForInput(String),
    Completed,
    Error(String),
}

/// Worker role configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRole {
    pub role_type: String,          // "backend", "frontend", "coherence", "simplify", or custom
    pub label: String,              // Display name
    pub default_cli: String,        // Default CLI for this role
    pub prompt_template: Option<String>, // Path to template or inline prompt
}

impl WorkerRole {
    pub fn new(role_type: &str, label: &str, default_cli: &str) -> Self {
        Self {
            role_type: role_type.to_string(),
            label: label.to_string(),
            default_cli: default_cli.to_string(),
            prompt_template: None,
        }
    }
}

impl Default for WorkerRole {
    fn default() -> Self {
        Self::new("general", "General", "claude")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub cli: String,              // "claude", "gemini", "opencode", "codex"
    pub model: Option<String>,    // "opus", "gemini-3-pro", etc.
    pub flags: Vec<String>,       // Additional CLI flags
    pub label: Option<String>,    // Display name
    pub role: Option<WorkerRole>, // Worker role assignment
    pub initial_prompt: Option<String>, // Prompt to inject on spawn
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            cli: "claude".to_string(),
            model: Some("opus".to_string()),
            flags: vec![],
            label: None,
            role: None,
            initial_prompt: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("Failed to create PTY: {0}")]
    CreateError(String),
    #[error("Failed to spawn command: {0}")]
    SpawnError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("PTY session not found: {0}")]
    NotFound(String),
}

// Wrapper to make the reader/writer Send
pub(crate) struct SendReader(Box<dyn Read + Send>);
pub(crate) struct SendWriter(Box<dyn Write + Send>);

unsafe impl Send for SendReader {}
unsafe impl Sync for SendReader {}
unsafe impl Send for SendWriter {}
unsafe impl Sync for SendWriter {}

// Wrapper to keep the master PTY alive and allow resize
pub(crate) struct MasterPtyHandle(Box<dyn portable_pty::MasterPty + Send>);
unsafe impl Send for MasterPtyHandle {}
unsafe impl Sync for MasterPtyHandle {}

impl MasterPtyHandle {
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        use portable_pty::PtySize;
        self.0
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))
    }
}

pub struct PtySession {
    pub role: AgentRole,
    pub status: Arc<parking_lot::RwLock<AgentStatus>>,
    writer: Arc<Mutex<SendWriter>>,
    reader: Arc<Mutex<SendReader>>,
    child: Arc<Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>>,
    master: Arc<Mutex<MasterPtyHandle>>,
}

// Make PtySession Send + Sync
unsafe impl Send for PtySession {}
unsafe impl Sync for PtySession {}

impl PtySession {
    pub fn new(
        _id: String,
        role: AgentRole,
        command: &str,
        args: &[&str],
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<Self, PtyError> {
        use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

        tracing::info!("Creating PTY session: command={} args={:?} cwd={:?}", command, args, cwd);

        let pty_system = NativePtySystem::default();

        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::CreateError(e.to_string()))?;

        // On Windows, create a batch file to avoid shell quoting issues
        // This is the same pattern used by /hive command
        let mut cmd = if cfg!(windows) {
            // Create temp batch file with the full command
            let batch_content = Self::create_batch_content(command, args);
            let batch_path = Self::write_temp_batch(&batch_content)?;

            tracing::info!("Created batch file: {} with content:\n{}", batch_path.display(), batch_content);

            let mut cmd = CommandBuilder::new("cmd.exe");
            cmd.args(&["/c", &batch_path.to_string_lossy()]);
            cmd
        } else {
            let mut cmd = CommandBuilder::new(command);
            cmd.args(args);
            cmd
        };

        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::SpawnError(e.to_string()))?;

        let writer = pty_pair
            .master
            .take_writer()
            .map_err(|e| PtyError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        let reader = pty_pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        // Keep the master alive - dropping it closes the PTY!
        let master = pty_pair.master;

        Ok(Self {
            role,
            status: Arc::new(parking_lot::RwLock::new(AgentStatus::Starting)),
            writer: Arc::new(Mutex::new(SendWriter(writer))),
            reader: Arc::new(Mutex::new(SendReader(reader))),
            child: Arc::new(Mutex::new(Some(child))),
            master: Arc::new(Mutex::new(MasterPtyHandle(master))),
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        tracing::debug!("PTY write: {} bytes: {:?}", data.len(), String::from_utf8_lossy(data));
        let mut writer = self.writer.lock();
        let result = writer.0.write_all(data);
        if let Err(ref e) = result {
            tracing::error!("PTY write_all failed: {}", e);
        }
        result?;
        let flush_result = writer.0.flush();
        if let Err(ref e) = flush_result {
            tracing::error!("PTY flush failed: {}", e);
        }
        flush_result?;
        tracing::debug!("PTY write complete");
        Ok(())
    }

    pub fn kill(&self) -> Result<(), PtyError> {
        let mut child = self.child.lock();
        if let Some(ref mut c) = *child {
            c.kill().map_err(|e| PtyError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
        }
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        let master = self.master.lock();
        master.resize(cols, rows)
    }

    pub fn get_reader(&self) -> Arc<Mutex<SendReader>> {
        Arc::clone(&self.reader)
    }

    /// Create batch file content for Windows command execution
    #[cfg(windows)]
    fn create_batch_content(command: &str, args: &[&str]) -> String {
        let mut lines = vec!["@echo off".to_string()];

        // Add CLI-specific environment variables
        if command == "opencode" {
            lines.push("set OPENCODE_YOLO=true".to_string());
        }

        // Build the command line with proper quoting
        let mut cmd_line = command.to_string();
        for arg in args {
            // Quote args that contain spaces or special characters
            if arg.contains(' ') || arg.contains('"') || arg.contains('&') || arg.contains('|') {
                // Escape any existing quotes and wrap in quotes
                let escaped = arg.replace('"', "\\\"");
                cmd_line.push_str(&format!(" \"{}\"", escaped));
            } else {
                cmd_line.push_str(&format!(" {}", arg));
            }
        }

        lines.push(cmd_line);
        lines.join("\r\n")
    }

    #[cfg(not(windows))]
    fn create_batch_content(_command: &str, _args: &[&str]) -> String {
        String::new()
    }

    /// Write a temporary batch file and return its path
    #[cfg(windows)]
    fn write_temp_batch(content: &str) -> Result<std::path::PathBuf, PtyError> {
        use std::io::Write;

        let temp_dir = std::env::temp_dir().join("hive-manager");
        std::fs::create_dir_all(&temp_dir)
            .map_err(|e| PtyError::CreateError(format!("Failed to create temp dir: {}", e)))?;

        // Generate unique filename
        let filename = format!("agent-{}.bat", uuid::Uuid::new_v4());
        let path = temp_dir.join(filename);

        let mut file = std::fs::File::create(&path)
            .map_err(|e| PtyError::CreateError(format!("Failed to create batch file: {}", e)))?;

        file.write_all(content.as_bytes())
            .map_err(|e| PtyError::CreateError(format!("Failed to write batch file: {}", e)))?;

        Ok(path)
    }

    #[cfg(not(windows))]
    fn write_temp_batch(_content: &str) -> Result<std::path::PathBuf, PtyError> {
        Err(PtyError::CreateError("Batch files only supported on Windows".to_string()))
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

// Helper function to read from SendReader
pub fn read_from_reader(reader: &Arc<Mutex<SendReader>>, buf: &mut [u8]) -> Result<usize, std::io::Error> {
    let mut r = reader.lock();
    r.0.read(buf)
}
