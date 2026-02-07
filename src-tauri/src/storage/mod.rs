use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use fs2::FileExt;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Pre-compiled regex for parsing coordination log lines (new format with message type)
static COORDINATION_LOG_REGEX_NEW: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\[([^\]]+)\] (TASK|PROGRESS|COMPLETION|ERROR|SYSTEM) ([^ ]+) → ([^:]+): (.*)$")
        .expect("Invalid coordination log regex (new format)")
});

/// Pre-compiled regex for parsing coordination log lines (legacy format without message type)
static COORDINATION_LOG_REGEX_LEGACY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\[([^\]]+)\] ([^ ]+) → ([^:]+): (.*)$")
        .expect("Invalid coordination log regex (legacy format)")
});

/// Regex for validating session IDs - only alphanumeric, dash, and underscore allowed
static SESSION_ID_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^[a-zA-Z0-9_-]+$").expect("Invalid session ID validation regex")
});

/// Validate a session ID to prevent path traversal attacks.
/// Session IDs must contain only alphanumeric characters, dashes, and underscores.
/// Returns an error if the session ID is invalid.
pub fn validate_session_id(session_id: &str) -> Result<(), StorageError> {
    // Check for empty session ID
    if session_id.is_empty() {
        return Err(StorageError::InvalidPath("Session ID cannot be empty".to_string()));
    }
    
    // Check for null bytes
    if session_id.contains('\0') {
        return Err(StorageError::InvalidPath("Session ID cannot contain null bytes".to_string()));
    }
    
    // Check for path traversal patterns
    if session_id.contains("..") {
        return Err(StorageError::InvalidPath("Session ID cannot contain '..'".to_string()));
    }
    
    // Validate against allowlist pattern (alphanumeric, dash, underscore only)
    if !SESSION_ID_REGEX.is_match(session_id) {
        return Err(StorageError::InvalidPath(
            "Session ID must contain only alphanumeric characters, dashes, and underscores".to_string()
        ));
    }
    
    // Check reasonable length (UUID is 36 chars, allow some buffer)
    if session_id.len() > 128 {
        return Err(StorageError::InvalidPath("Session ID is too long".to_string()));
    }
    
    Ok(())
}

use crate::coordination::{CoordinationMessage, MessageType};

/// Generate a deterministic ID for legacy learnings that lack one.
/// Uses UUID v5 (SHA-1 namespace hash) from concatenated fields so the same
/// entry always produces the same ID across reads.
fn generate_learning_id() -> String {
    // This default is only used when deserializing entries that lack an `id` field.
    // A post-deserialization fixup in the read paths replaces it with a content-based hash.
    String::new()
}

/// Produce a stable, content-based ID for a learning entry.
fn stable_learning_id(learning: &Learning) -> String {
    use uuid::Uuid;
    // Use the DNS namespace as a stable base (arbitrary but deterministic)
    // Include all fields to avoid collisions between entries that share date/session/task/insight
    let content = format!(
        "{}:{}:{}:{}:{}:{}:{}",
        learning.date,
        learning.session,
        learning.task,
        learning.outcome,
        learning.keywords.join(","),
        learning.insight,
        learning.files_touched.join(","),
    );
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, content.as_bytes()).to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Learning {
    #[serde(default = "generate_learning_id")]
    pub id: String,
    pub date: String,
    pub session: String,
    pub task: String,
    pub outcome: String,
    pub keywords: Vec<String>,
    pub insight: String,
    pub files_touched: Vec<String>,
}

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
        Self::new_with_base(base_dir)
    }

    /// Create a SessionStorage with a custom base directory (for testing)
    pub fn new_with_base(base_dir: PathBuf) -> Result<Self, StorageError> {
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
    /// Validates session_id to prevent path traversal attacks
    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        // Validate session_id - log warning if invalid but don't panic
        // (callers should validate before calling this)
        if let Err(e) = validate_session_id(session_id) {
            tracing::warn!("Invalid session_id passed to session_dir: {}", e);
        }
        self.sessions_dir().join(session_id)
    }
    
    /// Get path to a specific session directory with validation
    /// Returns an error if session_id is invalid
    pub fn session_dir_validated(&self, session_id: &str) -> Result<PathBuf, StorageError> {
        validate_session_id(session_id)?;
        Ok(self.sessions_dir().join(session_id))
    }

    /// Create a new session directory structure
    /// Validates session_id to prevent path traversal attacks
    pub fn create_session_dir(&self, session_id: &str) -> Result<PathBuf, StorageError> {
        validate_session_id(session_id)?;
        let session_dir = self.session_dir(session_id);

        // Create all subdirectories
        fs::create_dir_all(&session_dir)?;
        fs::create_dir_all(session_dir.join("state"))?;
        fs::create_dir_all(session_dir.join("coordination"))?;
        fs::create_dir_all(session_dir.join("prompts"))?;
        fs::create_dir_all(session_dir.join("logs"))?;
        fs::create_dir_all(session_dir.join("lessons"))?;
        fs::create_dir_all(session_dir.join("lessons").join("archive"))?;
        // Initialize empty state files
        fs::write(
            session_dir.join("state").join("workers.md"),
            "# Available Workers\n\nNo workers yet.\n",
        )?;
        fs::write(session_dir.join("state").join("hierarchy.json"), "[]")?;
        fs::write(session_dir.join("state").join("assignments.json"), "{}")?;

        // Initialize coordination files
        fs::write(
            session_dir.join("coordination").join("coordination.log"),
            "",
        )?;
        fs::write(
            session_dir.join("coordination").join("queen-inbox.md"),
            "# Queen Inbox\n\nNo messages yet.\n",
        )?;

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
    /// Validates session_id to prevent path traversal attacks
    pub fn load_session(&self, session_id: &str) -> Result<PersistedSession, StorageError> {
        validate_session_id(session_id)?;
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
                        SessionTypeInfo::Hive { worker_count } => {
                            format!("Hive ({})", worker_count)
                        }
                        SessionTypeInfo::Swarm { planner_count } => {
                            format!("Swarm ({})", planner_count)
                        }
                        SessionTypeInfo::Fusion { variants } => {
                            format!("Fusion ({})", variants.len())
                        }
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

        clis.insert(
            "claude".to_string(),
            CliConfig {
                command: "claude".to_string(),
                auto_approve_flag: Some("--dangerously-skip-permissions".to_string()),
                model_flag: Some("--model".to_string()),
                default_model: "opus-4-6".to_string(),
                env: None,
            },
        );

        clis.insert(
            "gemini".to_string(),
            CliConfig {
                command: "gemini".to_string(),
                auto_approve_flag: Some("-y".to_string()),
                model_flag: Some("-m".to_string()),
                default_model: "gemini-2.5-pro".to_string(),
                env: None,
            },
        );

        clis.insert(
            "opencode".to_string(),
            CliConfig {
                command: "opencode".to_string(),
                auto_approve_flag: None,
                model_flag: Some("-m".to_string()),
                default_model: "opencode/big-pickle".to_string(),
                env: Some({
                    let mut env = HashMap::new();
                    env.insert("OPENCODE_YOLO".to_string(), "true".to_string());
                    env
                }),
            },
        );

        clis.insert(
            "codex".to_string(),
            CliConfig {
                command: "codex".to_string(),
                auto_approve_flag: Some("--dangerously-bypass-approvals-and-sandbox".to_string()),
                model_flag: Some("-m".to_string()),
                default_model: "gpt-5.3-codex".to_string(),
                env: None,
            },
        );

        clis.insert(
            "cursor".to_string(),
            CliConfig {
                command: "wsl".to_string(),
                auto_approve_flag: Some("--force".to_string()),
                model_flag: None, // Cursor uses global model setting
                default_model: "composer-1".to_string(),
                env: None,
            },
        );

        clis.insert(
            "droid".to_string(),
            CliConfig {
                command: "droid".to_string(),
                auto_approve_flag: None, // Interactive mode - no auto-approve flag
                model_flag: None,        // Model selected via /model command in TUI
                default_model: "glm-4.7".to_string(),
                env: None,
            },
        );

        clis.insert(
            "qwen".to_string(),
            CliConfig {
                command: "qwen".to_string(),
                auto_approve_flag: Some("-y".to_string()),
                model_flag: Some("-m".to_string()),
                default_model: "qwen3-coder".to_string(),
                env: None,
            },
        );

        let mut default_roles = HashMap::new();
        default_roles.insert(
            "backend".to_string(),
            RoleDefaults {
                cli: "claude".to_string(),
                model: "opus-4-6".to_string(),
            },
        );
        default_roles.insert(
            "frontend".to_string(),
            RoleDefaults {
                cli: "gemini".to_string(),
                model: "gemini-2.5-pro".to_string(),
            },
        );
        default_roles.insert(
            "coherence".to_string(),
            RoleDefaults {
                cli: "droid".to_string(),
                model: "glm-4.7".to_string(),
            },
        );
        default_roles.insert(
            "simplify".to_string(),
            RoleDefaults {
                cli: "codex".to_string(),
                model: "gpt-5.3-codex".to_string(),
            },
        );

        AppConfig {
            clis,
            default_roles,
            api: ApiConfig {
                enabled: true,
                port: 18800,
            },
        }
    }

    /// Maximum content length for coordination log entries (prevents log bloat)
    const MAX_CONTENT_LENGTH: usize = 2000;

    /// Sanitize content for coordination log: strip newlines and limit length
    fn sanitize_content(content: &str) -> String {
        // Replace newlines with spaces to prevent log injection
        let sanitized = content
            .replace('\n', " ")
            .replace('\r', " ")
            .replace('\t', " ");

        // Collapse multiple spaces into single space
        let mut result = String::with_capacity(sanitized.len());
        let mut last_was_space = false;
        for c in sanitized.chars() {
            if c == ' ' {
                if !last_was_space {
                    result.push(c);
                }
                last_was_space = true;
            } else {
                result.push(c);
                last_was_space = false;
            }
        }

        // Trim and limit length
        let trimmed = result.trim();
        if trimmed.len() > Self::MAX_CONTENT_LENGTH {
            format!("{}...", &trimmed[..Self::MAX_CONTENT_LENGTH - 3])
        } else {
            trimmed.to_string()
        }
    }

    /// Convert MessageType to string for log format
    fn message_type_to_str(message_type: &MessageType) -> &'static str {
        match message_type {
            MessageType::Task => "TASK",
            MessageType::Progress => "PROGRESS",
            MessageType::Completion => "COMPLETION",
            MessageType::Error => "ERROR",
            MessageType::System => "SYSTEM",
        }
    }

    /// Parse MessageType from string
    fn str_to_message_type(s: &str) -> MessageType {
        match s.to_uppercase().as_str() {
            "TASK" => MessageType::Task,
            "PROGRESS" => MessageType::Progress,
            "COMPLETION" => MessageType::Completion,
            "ERROR" => MessageType::Error,
            "SYSTEM" => MessageType::System,
            _ => MessageType::Task, // Default fallback for legacy logs
        }
    }

    /// Append a message to the coordination log
    /// Uses file locking for concurrent write safety
    /// Validates session_id to prevent path traversal attacks
    pub fn append_coordination_log(
        &self,
        session_id: &str,
        message: &CoordinationMessage,
    ) -> Result<(), StorageError> {
        validate_session_id(session_id)?;
        let log_path = self
            .session_dir(session_id)
            .join("coordination")
            .join("coordination.log");

        // Sanitize content to prevent log injection
        let sanitized_content = Self::sanitize_content(&message.content);
        let message_type_str = Self::message_type_to_str(&message.message_type);

        // Format: [TIMESTAMP] TYPE FROM → TO: content
        let line = format!(
            "[{}] {} {} → {}: {}\n",
            message.timestamp.format("%Y-%m-%dT%H:%M:%SZ"),
            message_type_str,
            message.from,
            message.to,
            sanitized_content
        );

        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        // Acquire exclusive lock for write safety (blocks until lock acquired)
        file.lock_exclusive().map_err(|e| {
            StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to acquire file lock: {}", e),
            ))
        })?;

        // Write the line
        let result = file.write_all(line.as_bytes());

        // Unlock (happens automatically on drop, but explicit is clearer)
        let _ = file.unlock();

        result?;
        Ok(())
    }

    /// Read the coordination log
    /// Validates session_id to prevent path traversal attacks
    pub fn read_coordination_log(
        &self,
        session_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<CoordinationMessage>, StorageError> {
        validate_session_id(session_id)?;
        let log_path = self
            .session_dir(session_id)
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

    /// Parse a coordination log line using pre-compiled static regexes
    fn parse_coordination_line(line: &str) -> Option<CoordinationMessage> {
        // New format: [2024-02-03T18:52:34Z] TYPE FROM → TO: content
        // Legacy format: [2024-02-03T18:52:34Z] FROM → TO: content

        // Try new format first (with message type) using pre-compiled regex
        if let Some(caps) = COORDINATION_LOG_REGEX_NEW.captures(line) {
            let timestamp = DateTime::parse_from_rfc3339(&caps[1])
                .ok()?
                .with_timezone(&Utc);

            return Some(CoordinationMessage {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp,
                from: caps[3].to_string(),
                to: caps[4].to_string(),
                content: caps[5].to_string(),
                message_type: Self::str_to_message_type(&caps[2]),
            });
        }

        // Fall back to legacy format (no message type - defaults to Task)
        let caps = COORDINATION_LOG_REGEX_LEGACY.captures(line)?;

        let timestamp = DateTime::parse_from_rfc3339(&caps[1])
            .ok()?
            .with_timezone(&Utc);

        Some(CoordinationMessage {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp,
            from: caps[2].to_string(),
            to: caps[3].to_string(),
            content: caps[4].to_string(),
            message_type: MessageType::Task,
        })
    }

    fn ai_docs_dir(project_path: &Path) -> PathBuf {
        project_path.join(".ai-docs")
    }

    /// Get the session-scoped lessons directory
    /// Stores learnings and project DNA in .hive-manager/{session_id}/lessons/
    /// Validates session_id to prevent path traversal attacks
    fn session_lessons_dir(&self, session_id: &str) -> PathBuf {
        // Validation is performed by session_dir
        // For per-session lessons, store in %APPDATA%/hive-manager/sessions/{session_id}/lessons/
        // This allows multi-project sessions without conflicts
        self.session_dir(session_id).join("lessons")
    }
    
    /// Get the session-scoped lessons directory with validation
    /// Returns an error if session_id is invalid
    fn session_lessons_dir_validated(&self, session_id: &str) -> Result<PathBuf, StorageError> {
        validate_session_id(session_id)?;
        Ok(self.session_dir(session_id).join("lessons"))
    }

    /// Append a learning to the .ai-docs/learnings.jsonl file (project-scoped, legacy)
    /// DEPRECATED: Use append_learning_session for new code
    pub fn append_learning(
        &self,
        project_path: &Path,
        learning: &Learning,
    ) -> Result<(), StorageError> {
        let ai_docs_dir = Self::ai_docs_dir(project_path);
        fs::create_dir_all(&ai_docs_dir)?;
        // Also ensure archive folder exists for /curate-learnings skill
        fs::create_dir_all(ai_docs_dir.join("archive"))?;
        let learnings_file = ai_docs_dir.join("learnings.jsonl");

        let mut json = serde_json::to_string(learning)?;
        json.push('\n');

        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(learnings_file)?;

        file.write_all(json.as_bytes())?;

        Ok(())
    }

    /// Append a learning to the session-scoped lessons directory
    /// Stores in .hive-manager/{session_id}/lessons/learnings.jsonl
    /// Validates session_id to prevent path traversal attacks
    pub fn append_learning_session(
        &self,
        session_id: &str,
        learning: &Learning,
    ) -> Result<(), StorageError> {
        validate_session_id(session_id)?;
        let lessons_dir = self.session_lessons_dir(session_id);
        fs::create_dir_all(&lessons_dir)?;
        // Also ensure archive folder exists
        fs::create_dir_all(lessons_dir.join("archive"))?;
        let learnings_file = lessons_dir.join("learnings.jsonl");

        let mut json = serde_json::to_string(learning)?;
        json.push('\n');

        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(learnings_file)?;

        file.write_all(json.as_bytes())?;

        Ok(())
    }

    /// Read all learnings from .ai-docs/learnings.jsonl (project-scoped, legacy)
    /// DEPRECATED: Use read_learnings_session for new code
    pub fn read_learnings(&self, project_path: &Path) -> Result<Vec<Learning>, StorageError> {
        let learnings_file = Self::ai_docs_dir(project_path).join("learnings.jsonl");

        if !learnings_file.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(learnings_file)?;
        let mut learnings = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Learning>(line) {
                Ok(mut learning) => {
                    if learning.id.is_empty() {
                        learning.id = stable_learning_id(&learning);
                    }
                    learnings.push(learning);
                }
                Err(e) => {
                    tracing::warn!("Failed to parse learning line: {}. Error: {}", line, e);
                }
            }
        }

        Ok(learnings)
    }

    /// Delete a learning by ID from the session-scoped learnings file
    /// Uses temp-file + rename for atomicity
    /// Validates session_id to prevent path traversal attacks
    pub fn delete_learning_session(
        &self,
        session_id: &str,
        learning_id: &str,
    ) -> Result<bool, StorageError> {
        validate_session_id(session_id)?;
        let learnings_file = self.session_lessons_dir(session_id).join("learnings.jsonl");

        if !learnings_file.exists() {
            return Ok(false);
        }

        let content = fs::read_to_string(&learnings_file)?;
        let mut found = false;
        let mut remaining_lines = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Learning>(line) {
                Ok(mut learning) => {
                    // Apply same fixup as read path for entries without persisted ID
                    if learning.id.is_empty() {
                        learning.id = stable_learning_id(&learning);
                    }
                    if learning.id == learning_id {
                        found = true;
                        // Skip this line (delete it)
                    } else {
                        remaining_lines.push(line);
                    }
                }
                Err(_) => {
                    remaining_lines.push(line);
                }
            }
        }

        if !found {
            return Ok(false);
        }

        // Write to temp file then rename for atomicity using tempfile crate
        let lessons_dir = self.session_lessons_dir(session_id);
        let mut temp =
            tempfile::NamedTempFile::new_in(&lessons_dir).map_err(|e| StorageError::Io(e))?;
        for line in &remaining_lines {
            writeln!(temp, "{}", line).map_err(|e| StorageError::Io(e))?;
        }
        temp.persist(&learnings_file)
            .map_err(|e| StorageError::Io(e.error))?;

        Ok(true)
    }

    /// Read all learnings from the session-scoped lessons directory
    /// Reads from .hive-manager/{session_id}/lessons/learnings.jsonl
    /// Validates session_id to prevent path traversal attacks
    pub fn read_learnings_session(&self, session_id: &str) -> Result<Vec<Learning>, StorageError> {
        validate_session_id(session_id)?;
        let learnings_file = self.session_lessons_dir(session_id).join("learnings.jsonl");

        if !learnings_file.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(learnings_file)?;
        let mut learnings = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Learning>(line) {
                Ok(mut learning) => {
                    if learning.id.is_empty() {
                        learning.id = stable_learning_id(&learning);
                    }
                    learnings.push(learning);
                }
                Err(e) => {
                    tracing::warn!("Failed to parse learning line: {}. Error: {}", line, e);
                }
            }
        }

        Ok(learnings)
    }

    /// Read .ai-docs/project-dna.md content (project-scoped, legacy)
    /// DEPRECATED: Use read_project_dna_session for new code
    pub fn read_project_dna(&self, project_path: &Path) -> Result<String, StorageError> {
        let project_dna_file = Self::ai_docs_dir(project_path).join("project-dna.md");
        if !project_dna_file.exists() {
            return Ok(String::new());
        }
        Ok(fs::read_to_string(project_dna_file)?)
    }

    /// Read project DNA from the session-scoped lessons directory
    /// Reads from .hive-manager/{session_id}/lessons/project-dna.md
    /// Validates session_id to prevent path traversal attacks
    pub fn read_project_dna_session(&self, session_id: &str) -> Result<String, StorageError> {
        validate_session_id(session_id)?;
        let project_dna_file = self.session_lessons_dir(session_id).join("project-dna.md");
        if !project_dna_file.exists() {
            return Ok(String::new());
        }
        Ok(fs::read_to_string(project_dna_file)?)
    }

    /// Save curated project DNA to .ai-docs/project-dna.md (project-scoped, legacy)
    /// DEPRECATED: Use save_project_dna_session for new code
    #[allow(dead_code)]
    pub fn save_project_dna(&self, project_path: &Path, content: &str) -> Result<(), StorageError> {
        let ai_docs_dir = Self::ai_docs_dir(project_path);
        fs::create_dir_all(&ai_docs_dir)?;
        let project_dna_file = ai_docs_dir.join("project-dna.md");
        fs::write(project_dna_file, content)?;
        Ok(())
    }

    /// Save curated project DNA to the session-scoped lessons directory
    /// Saves to .hive-manager/{session_id}/lessons/project-dna.md
    /// Validates session_id to prevent path traversal attacks
    pub fn save_project_dna_session(
        &self,
        session_id: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        validate_session_id(session_id)?;
        let lessons_dir = self.session_lessons_dir(session_id);
        fs::create_dir_all(&lessons_dir)?;
        let project_dna_file = lessons_dir.join("project-dna.md");
        fs::write(project_dna_file, content)?;
        Ok(())
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
    /// HTTP API configuration
    #[serde(default)]
    pub api: ApiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub enabled: bool,
    pub port: u16,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Enabled by default for Queen to spawn workers
            port: 18800,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_storage() -> (SessionStorage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(temp_dir.path().to_path_buf()).unwrap();
        (storage, temp_dir)
    }

    #[test]
    fn test_learning_serialization_roundtrip() {
        let learning = Learning {
            id: "test-id-123".to_string(),
            date: "2024-01-01".to_string(),
            session: "test-session".to_string(),
            task: "test task".to_string(),
            outcome: "success".to_string(),
            keywords: vec!["rust".to_string(), "api".to_string()],
            insight: "test insight".to_string(),
            files_touched: vec!["src/file.rs".to_string()],
        };

        let json = serde_json::to_string(&learning).unwrap();
        let deserialized: Learning = serde_json::from_str(&json).unwrap();

        assert_eq!(learning.id, deserialized.id);
        assert_eq!(learning.date, deserialized.date);
        assert_eq!(learning.session, deserialized.session);
        assert_eq!(learning.task, deserialized.task);
        assert_eq!(learning.outcome, deserialized.outcome);
        assert_eq!(learning.keywords, deserialized.keywords);
        assert_eq!(learning.insight, deserialized.insight);
        assert_eq!(learning.files_touched, deserialized.files_touched);
    }

    #[test]
    fn test_learning_deserialization_with_id() {
        let json = r#"{
            "id": "custom-id-456",
            "date": "2024-01-02",
            "session": "session-1",
            "task": "task 1",
            "outcome": "partial",
            "keywords": ["test"],
            "insight": "insight 1",
            "files_touched": ["file1.rs"]
        }"#;

        let learning: Learning = serde_json::from_str(json).unwrap();
        assert_eq!(learning.id, "custom-id-456");
        assert_eq!(learning.date, "2024-01-02");
        assert_eq!(learning.session, "session-1");
    }

    #[test]
    fn test_learning_deserialization_without_id_backward_compat() {
        // Test backward compatibility - JSON without id field deserializes to empty string
        // (fixup happens in read paths via stable_learning_id)
        let json = r#"{
            "date": "2024-01-03",
            "session": "session-2",
            "task": "task 2",
            "outcome": "failed",
            "keywords": [],
            "insight": "insight 2",
            "files_touched": []
        }"#;

        let mut learning: Learning = serde_json::from_str(json).unwrap();
        // Raw deserialization yields empty ID
        assert!(learning.id.is_empty());
        assert_eq!(learning.date, "2024-01-03");
        assert_eq!(learning.session, "session-2");

        // Fixup: apply stable content-based hash
        learning.id = stable_learning_id(&learning);
        assert!(!learning.id.is_empty());
        assert_eq!(learning.id.len(), 36); // UUID format

        // Deserializing the same content again should produce the same ID
        let learning2: Learning = serde_json::from_str(json).unwrap();
        let id2 = stable_learning_id(&learning2);
        assert_eq!(learning.id, id2); // Deterministic
    }

    #[test]
    fn test_append_and_read_learnings_session() {
        let (storage, _temp_dir) = create_test_storage();
        let session_id = "test-session-append-read";

        // Create session directory structure
        storage.create_session_dir(session_id).unwrap();

        let learning = Learning {
            id: "test-learning-1".to_string(),
            date: "2024-01-01".to_string(),
            session: session_id.to_string(),
            task: "test task".to_string(),
            outcome: "success".to_string(),
            keywords: vec!["test".to_string()],
            insight: "test insight".to_string(),
            files_touched: vec!["src/file.rs".to_string()],
        };

        // Append learning
        storage
            .append_learning_session(session_id, &learning)
            .unwrap();

        // Read learnings
        let learnings = storage.read_learnings_session(session_id).unwrap();
        assert_eq!(learnings.len(), 1);
        assert_eq!(learnings[0].id, learning.id);
        assert_eq!(learnings[0].task, learning.task);
        assert_eq!(learnings[0].insight, learning.insight);
    }

    #[test]
    fn test_delete_learning_by_id() {
        let (storage, _temp_dir) = create_test_storage();
        let session_id = "test-session-delete";

        storage.create_session_dir(session_id).unwrap();

        // Append 3 learnings
        let learning1 = Learning {
            id: "learning-1".to_string(),
            date: "2024-01-01".to_string(),
            session: session_id.to_string(),
            task: "task 1".to_string(),
            outcome: "success".to_string(),
            keywords: vec![],
            insight: "insight 1".to_string(),
            files_touched: vec![],
        };

        let learning2 = Learning {
            id: "learning-2".to_string(),
            date: "2024-01-02".to_string(),
            session: session_id.to_string(),
            task: "task 2".to_string(),
            outcome: "success".to_string(),
            keywords: vec![],
            insight: "insight 2".to_string(),
            files_touched: vec![],
        };

        let learning3 = Learning {
            id: "learning-3".to_string(),
            date: "2024-01-03".to_string(),
            session: session_id.to_string(),
            task: "task 3".to_string(),
            outcome: "success".to_string(),
            keywords: vec![],
            insight: "insight 3".to_string(),
            files_touched: vec![],
        };

        storage
            .append_learning_session(session_id, &learning1)
            .unwrap();
        storage
            .append_learning_session(session_id, &learning2)
            .unwrap();
        storage
            .append_learning_session(session_id, &learning3)
            .unwrap();

        // Delete the middle one
        let deleted = storage
            .delete_learning_session(session_id, "learning-2")
            .unwrap();
        assert!(deleted);

        // Read learnings - should only have 2
        let learnings = storage.read_learnings_session(session_id).unwrap();
        assert_eq!(learnings.len(), 2);
        assert!(learnings.iter().any(|l| l.id == "learning-1"));
        assert!(learnings.iter().any(|l| l.id == "learning-3"));
        assert!(!learnings.iter().any(|l| l.id == "learning-2"));
    }

    #[test]
    fn test_delete_nonexistent_learning() {
        let (storage, _temp_dir) = create_test_storage();
        let session_id = "test-session-delete-nonexistent";

        storage.create_session_dir(session_id).unwrap();

        // Append a learning
        let learning = Learning {
            id: "learning-1".to_string(),
            date: "2024-01-01".to_string(),
            session: session_id.to_string(),
            task: "task 1".to_string(),
            outcome: "success".to_string(),
            keywords: vec![],
            insight: "insight 1".to_string(),
            files_touched: vec![],
        };

        storage
            .append_learning_session(session_id, &learning)
            .unwrap();

        // Try to delete non-existent learning
        let deleted = storage
            .delete_learning_session(session_id, "nonexistent-id")
            .unwrap();
        assert!(!deleted);

        // Verify original learning still exists
        let learnings = storage.read_learnings_session(session_id).unwrap();
        assert_eq!(learnings.len(), 1);
    }

    #[test]
    fn test_read_learnings_empty_file() {
        let (storage, _temp_dir) = create_test_storage();
        let session_id = "test-session-empty";

        storage.create_session_dir(session_id).unwrap();

        // Read from non-existent file should return empty vec
        let learnings = storage.read_learnings_session(session_id).unwrap();
        assert_eq!(learnings.len(), 0);
    }

    #[test]
    fn test_read_learnings_skips_malformed_lines() {
        let (storage, _temp_dir) = create_test_storage();
        let session_id = "test-session-malformed";

        storage.create_session_dir(session_id).unwrap();

        let learnings_file = storage
            .session_lessons_dir(session_id)
            .join("learnings.jsonl");

        // Write a file with valid and invalid lines
        let content = r#"{"id":"valid-1","date":"2024-01-01","session":"test","task":"task1","outcome":"success","keywords":[],"insight":"insight1","files_touched":[]}
invalid json line
{"id":"valid-2","date":"2024-01-02","session":"test","task":"task2","outcome":"success","keywords":[],"insight":"insight2","files_touched":[]}
{invalid}
{"id":"valid-3","date":"2024-01-03","session":"test","task":"task3","outcome":"success","keywords":[],"insight":"insight3","files_touched":[]}
"#;

        std::fs::write(&learnings_file, content).unwrap();

        // Read should skip malformed lines and return only valid ones
        let learnings = storage.read_learnings_session(session_id).unwrap();
        assert_eq!(learnings.len(), 3);
        assert_eq!(learnings[0].id, "valid-1");
        assert_eq!(learnings[1].id, "valid-2");
        assert_eq!(learnings[2].id, "valid-3");
    }
    
    // ============ Session ID Validation Tests ============
    
    #[test]
    fn test_validate_session_id_valid() {
        // Valid session IDs
        assert!(validate_session_id("abc123").is_ok());
        assert!(validate_session_id("test-session-1").is_ok());
        assert!(validate_session_id("my_session").is_ok());
        assert!(validate_session_id("a1b2c3d4-e5f6-7890-abcd-ef1234567890").is_ok());
        assert!(validate_session_id("UPPERCASE").is_ok());
        assert!(validate_session_id("MixedCase123").is_ok());
    }
    
    #[test]
    fn test_validate_session_id_path_traversal() {
        // Path traversal attempts
        assert!(validate_session_id("..").is_err());
        assert!(validate_session_id("../etc/passwd").is_err());
        assert!(validate_session_id("..\\windows\\system32").is_err());
        assert!(validate_session_id("foo/../bar").is_err());
        assert!(validate_session_id("valid..invalid").is_err());
    }
    
    #[test]
    fn test_validate_session_id_invalid_chars() {
        // Invalid characters
        assert!(validate_session_id("path/slash").is_err());
        assert!(validate_session_id("path\\backslash").is_err());
        assert!(validate_session_id("with space").is_err());
        assert!(validate_session_id("special@char").is_err());
        assert!(validate_session_id("emoji🎉").is_err());
        assert!(validate_session_id("null\0byte").is_err());
    }
    
    #[test]
    fn test_validate_session_id_empty() {
        assert!(validate_session_id("").is_err());
    }
    
    #[test]
    fn test_validate_session_id_too_long() {
        let long_id = "a".repeat(200);
        assert!(validate_session_id(&long_id).is_err());
    }
    
    #[test]
    fn test_create_session_dir_validates_id() {
        let (storage, _temp_dir) = create_test_storage();
        
        // Valid session ID should work
        assert!(storage.create_session_dir("valid-session-123").is_ok());
        
        // Path traversal should fail
        assert!(storage.create_session_dir("../escape").is_err());
        assert!(storage.create_session_dir("..\\escape").is_err());
        assert!(storage.create_session_dir("a/b/c").is_err());
    }
    
    #[test]
    fn test_append_learning_session_validates_id() {
        let (storage, _temp_dir) = create_test_storage();
        
        let learning = Learning {
            id: "test-1".to_string(),
            date: "2024-01-01".to_string(),
            session: "test".to_string(),
            task: "task".to_string(),
            outcome: "success".to_string(),
            keywords: vec![],
            insight: "insight".to_string(),
            files_touched: vec![],
        };
        
        // Path traversal should fail
        assert!(storage.append_learning_session("../escape", &learning).is_err());
        assert!(storage.append_learning_session("a/b", &learning).is_err());
        assert!(storage.append_learning_session("null\0byte", &learning).is_err());
    }
}
