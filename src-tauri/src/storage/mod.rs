use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::coordination::CoordinationMessage;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

/// Summary of a session for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub session_type: String,
    pub project_path: String,
    pub created_at: DateTime<Utc>,
    pub agent_count: usize,
    pub state: String,
}

/// Persisted session metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSession {
    pub id: String,
    pub session_type: SessionTypeInfo,
    pub project_path: String,
    pub created_at: DateTime<Utc>,
    pub agents: Vec<PersistedAgentInfo>,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionTypeInfo {
    Hive { worker_count: u8 },
    Swarm { planner_count: u8 },
    Fusion { variants: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAgentInfo {
    pub id: String,
    pub role: String,
    pub config: PersistedAgentConfig,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAgentConfig {
    pub cli: String,
    pub model: Option<String>,
    pub flags: Vec<String>,
    pub label: Option<String>,
    pub role_type: Option<String>,
    pub initial_prompt: Option<String>,
}

/// Manages session storage in %APPDATA%/hive-manager
pub struct SessionStorage {
    base_dir: PathBuf,
}

impl SessionStorage {
    /// Create a new SessionStorage, initializing the base directory if needed
    pub fn new() -> Result<Self, StorageError> {
        let base_dir = Self::get_app_data_dir()?;
        fs::create_dir_all(&base_dir)?;
        fs::create_dir_all(base_dir.join("templates").join("roles"))?;
        fs::create_dir_all(base_dir.join("sessions"))?;

        // Create default config if it doesn't exist
        let config_path = base_dir.join("config.json");
        if !config_path.exists() {
            let default_config = Self::default_config();
            fs::write(&config_path, serde_json::to_string_pretty(&default_config)?)?;
        }

        Ok(Self { base_dir })
    }

    /// Get the app data directory path
    fn get_app_data_dir() -> Result<PathBuf, StorageError> {
        #[cfg(windows)]
        {
            std::env::var("APPDATA")
                .map(|p| PathBuf::from(p).join("hive-manager"))
                .map_err(|_| StorageError::InvalidPath("APPDATA not set".to_string()))
        }
        #[cfg(not(windows))]
        {
            std::env::var("HOME")
                .map(|p| PathBuf::from(p).join(".config").join("hive-manager"))
                .map_err(|_| StorageError::InvalidPath("HOME not set".to_string()))
        }
    }

    /// Get the base directory path
    #[allow(dead_code)]
    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }

    /// Get path to templates directory
    #[allow(dead_code)]
    pub fn templates_dir(&self) -> PathBuf {
        self.base_dir.join("templates")
    }

    /// Get path to sessions directory
    pub fn sessions_dir(&self) -> PathBuf {
        self.base_dir.join("sessions")
    }

    /// Get path to a specific session directory
    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(session_id)
    }

    /// Create a new session directory structure
    pub fn create_session_dir(&self, session_id: &str) -> Result<PathBuf, StorageError> {
        let session_dir = self.session_dir(session_id);

        // Create all subdirectories
        fs::create_dir_all(&session_dir)?;
        fs::create_dir_all(session_dir.join("state"))?;
        fs::create_dir_all(session_dir.join("coordination"))?;
        fs::create_dir_all(session_dir.join("prompts"))?;
        fs::create_dir_all(session_dir.join("logs"))?;

        // Initialize empty state files
        fs::write(session_dir.join("state").join("workers.md"), "# Available Workers\n\nNo workers yet.\n")?;
        fs::write(session_dir.join("state").join("hierarchy.json"), "[]")?;
        fs::write(session_dir.join("state").join("assignments.json"), "{}")?;

        // Initialize coordination files
        fs::write(session_dir.join("coordination").join("coordination.log"), "")?;
        fs::write(session_dir.join("coordination").join("queen-inbox.md"), "# Queen Inbox\n\nNo messages yet.\n")?;

        Ok(session_dir)
    }

    /// Save session metadata to disk
    #[allow(dead_code)]
    pub fn save_session(&self, session: &PersistedSession) -> Result<(), StorageError> {
        let session_dir = self.session_dir(&session.id);
        if !session_dir.exists() {
            self.create_session_dir(&session.id)?;
        }

        let session_file = session_dir.join("session.json");
        let json = serde_json::to_string_pretty(session)?;
        fs::write(session_file, json)?;

        Ok(())
    }

    /// Load session metadata from disk
    pub fn load_session(&self, session_id: &str) -> Result<PersistedSession, StorageError> {
        let session_file = self.session_dir(session_id).join("session.json");
        if !session_file.exists() {
            return Err(StorageError::SessionNotFound(session_id.to_string()));
        }

        let json = fs::read_to_string(session_file)?;
        let session: PersistedSession = serde_json::from_str(&json)?;

        Ok(session)
    }

    /// List all stored sessions
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>, StorageError> {
        let sessions_dir = self.sessions_dir();
        let mut summaries = Vec::new();

        if !sessions_dir.exists() {
            return Ok(summaries);
        }

        for entry in fs::read_dir(sessions_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let session_id = entry.file_name().to_string_lossy().to_string();
                if let Ok(session) = self.load_session(&session_id) {
                    let session_type = match &session.session_type {
                        SessionTypeInfo::Hive { worker_count } => format!("Hive ({})", worker_count),
                        SessionTypeInfo::Swarm { planner_count } => format!("Swarm ({})", planner_count),
                        SessionTypeInfo::Fusion { variants } => format!("Fusion ({})", variants.len()),
                    };

                    summaries.push(SessionSummary {
                        id: session.id,
                        session_type,
                        project_path: session.project_path,
                        created_at: session.created_at,
                        agent_count: session.agents.len(),
                        state: session.state,
                    });
                }
            }
        }

        // Sort by created_at descending
        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(summaries)
    }

    /// Delete a session and all its files
    #[allow(dead_code)]
    pub fn delete_session(&self, session_id: &str) -> Result<(), StorageError> {
        let session_dir = self.session_dir(session_id);
        if session_dir.exists() {
            fs::remove_dir_all(session_dir)?;
        }
        Ok(())
    }

    /// Get the config file path
    pub fn config_path(&self) -> PathBuf {
        self.base_dir.join("config.json")
    }

    /// Load the app config
    pub fn load_config(&self) -> Result<AppConfig, StorageError> {
        let config_path = self.config_path();
        if !config_path.exists() {
            let default_config = Self::default_config();
            fs::write(&config_path, serde_json::to_string_pretty(&default_config)?)?;
            return Ok(default_config);
        }

        let json = fs::read_to_string(config_path)?;
        let config: AppConfig = serde_json::from_str(&json)?;
        Ok(config)
    }

    /// Save the app config
    pub fn save_config(&self, config: &AppConfig) -> Result<(), StorageError> {
        let json = serde_json::to_string_pretty(config)?;
        fs::write(self.config_path(), json)?;
        Ok(())
    }

    /// Get default config with CLI registry
    fn default_config() -> AppConfig {
        let mut clis = HashMap::new();

        clis.insert("claude".to_string(), CliConfig {
            command: "claude".to_string(),
            auto_approve_flag: Some("--dangerously-skip-permissions".to_string()),
            model_flag: Some("--model".to_string()),
            default_model: "opus".to_string(),
            env: None,
        });

        clis.insert("gemini".to_string(), CliConfig {
            command: "gemini".to_string(),
            auto_approve_flag: Some("-y".to_string()),
            model_flag: Some("-m".to_string()),
            default_model: "gemini-2.5-pro".to_string(),
            env: None,
        });

        clis.insert("opencode".to_string(), CliConfig {
            command: "opencode".to_string(),
            auto_approve_flag: None,
            model_flag: Some("-m".to_string()),
            default_model: "grok".to_string(),
            env: Some({
                let mut env = HashMap::new();
                env.insert("OPENCODE_YOLO".to_string(), "true".to_string());
                env
            }),
        });

        clis.insert("codex".to_string(), CliConfig {
            command: "codex".to_string(),
            auto_approve_flag: Some("--dangerously-bypass-approvals-and-sandbox".to_string()),
            model_flag: Some("-m".to_string()),
            default_model: "gpt-5.2".to_string(),
            env: None,
        });

        clis.insert("cursor".to_string(), CliConfig {
            command: "wsl".to_string(),
            auto_approve_flag: Some("--force".to_string()),
            model_flag: None,
            default_model: "opus-4.5".to_string(),
            env: None,
        });

        clis.insert("droid".to_string(), CliConfig {
            command: "droid".to_string(),
            auto_approve_flag: Some("--skip-permissions-unsafe".to_string()),
            model_flag: Some("-m".to_string()),
            default_model: "claude-opus-4-5-20251101".to_string(),
            env: None,
        });

        clis.insert("qwen".to_string(), CliConfig {
            command: "qwen".to_string(),
            auto_approve_flag: Some("-y".to_string()),
            model_flag: Some("-m".to_string()),
            default_model: "qwen3-coder".to_string(),
            env: None,
        });

        let mut default_roles = HashMap::new();
        default_roles.insert("backend".to_string(), RoleDefaults {
            cli: "claude".to_string(),
            model: "opus".to_string(),
        });
        default_roles.insert("frontend".to_string(), RoleDefaults {
            cli: "gemini".to_string(),
            model: "gemini-2.5-pro".to_string(),
        });
        default_roles.insert("coherence".to_string(), RoleDefaults {
            cli: "opencode".to_string(),
            model: "grok".to_string(),
        });
        default_roles.insert("simplify".to_string(), RoleDefaults {
            cli: "codex".to_string(),
            model: "gpt-5.2".to_string(),
        });

        AppConfig {
            clis,
            default_roles,
        }
    }

    /// Append a message to the coordination log
    pub fn append_coordination_log(
        &self,
        session_id: &str,
        message: &CoordinationMessage,
    ) -> Result<(), StorageError> {
        let log_path = self.session_dir(session_id)
            .join("coordination")
            .join("coordination.log");

        let line = format!(
            "[{}] {} → {}: {}\n",
            message.timestamp.format("%Y-%m-%dT%H:%M:%SZ"),
            message.from,
            message.to,
            message.content
        );

        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;

        file.write_all(line.as_bytes())?;

        Ok(())
    }

    /// Read the coordination log
    pub fn read_coordination_log(
        &self,
        session_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<CoordinationMessage>, StorageError> {
        let log_path = self.session_dir(session_id)
            .join("coordination")
            .join("coordination.log");

        if !log_path.exists() {
            return Ok(vec![]);
        }

        let content = fs::read_to_string(log_path)?;
        let lines: Vec<&str> = content.lines().collect();

        let lines_to_parse = if let Some(limit) = limit {
            lines.iter().rev().take(limit).rev().collect::<Vec<_>>()
        } else {
            lines.iter().collect()
        };

        let mut messages = Vec::new();
        for line in lines_to_parse {
            if let Some(msg) = Self::parse_coordination_line(line) {
                messages.push(msg);
            }
        }

        Ok(messages)
    }

    /// Parse a coordination log line
    fn parse_coordination_line(line: &str) -> Option<CoordinationMessage> {
        // Format: [2024-02-03T18:52:34Z] FROM → TO: content
        let re = regex::Regex::new(r"^\[([^\]]+)\] ([^ ]+) → ([^:]+): (.*)$").ok()?;
        let caps = re.captures(line)?;

        let timestamp = DateTime::parse_from_rfc3339(&caps[1])
            .ok()?
            .with_timezone(&Utc);

        Some(CoordinationMessage {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp,
            from: caps[2].to_string(),
            to: caps[3].to_string(),
            content: caps[4].to_string(),
            message_type: crate::coordination::MessageType::Task,
        })
    }
}

impl Default for SessionStorage {
    fn default() -> Self {
        Self::new().expect("Failed to initialize session storage")
    }
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub clis: HashMap<String, CliConfig>,
    pub default_roles: HashMap<String, RoleDefaults>,
}

/// CLI configuration for a specific agent CLI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    pub command: String,
    pub auto_approve_flag: Option<String>,
    pub model_flag: Option<String>,
    pub default_model: String,
    pub env: Option<HashMap<String, String>>,
}

/// Default settings for a role
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDefaults {
    pub cli: String,
    pub model: String,
}
