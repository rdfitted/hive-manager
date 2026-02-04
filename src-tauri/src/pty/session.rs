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
    WaitingForInput,
    Completed,
    Error(String),
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

pub struct PtySession {
    pub role: AgentRole,
    pub status: AgentStatus,
    writer: Arc<Mutex<SendWriter>>,
    reader: Arc<Mutex<SendReader>>,
    child: Arc<Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>>,
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

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);

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

        // Drop the pty_pair - we only need the reader/writer now
        drop(pty_pair);

        Ok(Self {
            role,
            status: AgentStatus::Starting,
            writer: Arc::new(Mutex::new(SendWriter(writer))),
            reader: Arc::new(Mutex::new(SendReader(reader))),
            child: Arc::new(Mutex::new(Some(child))),
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut writer = self.writer.lock();
        writer.0.write_all(data)?;
        writer.0.flush()?;
        Ok(())
    }

    pub fn kill(&self) -> Result<(), PtyError> {
        let mut child = self.child.lock();
        if let Some(ref mut c) = *child {
            c.kill().map_err(|e| PtyError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
        }
        Ok(())
    }

    pub fn get_reader(&self) -> Arc<Mutex<SendReader>> {
        Arc::clone(&self.reader)
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
