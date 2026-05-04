use std::collections::{hash_map::DefaultHasher, HashMap};
use std::fs;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::coordination::CoordinationMessage;
use crate::domain::{ArtifactBundle, ResolverOutput};
use crate::session::cell_status::PRIMARY_CELL_ID;
use crate::session::DEFAULT_MAX_QA_ITERATIONS;
use crate::templates::SessionTemplate;

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

fn deserialize_optional_trimmed_string<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }))
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub timestamp: DateTime<Utc>,
    pub from: String,
    pub content: String,
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
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub session_type: String,
    pub project_path: String,
    pub created_at: DateTime<Utc>,
    /// Best-known recent activity for dashboards (falls back to `created_at` when unset in storage).
    pub last_activity_at: DateTime<Utc>,
    pub agent_count: usize,
    pub state: String,
}

/// Persisted session metadata
#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct PersistedSession {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub session_type: SessionTypeInfo,
    pub project_path: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_activity_at: Option<DateTime<Utc>>,
    pub agents: Vec<PersistedAgentInfo>,
    pub state: String,
    #[serde(default = "default_cli")]
    pub default_cli: String,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub qa_workers: Vec<crate::session::QaWorkerConfig>,
    #[serde(default = "default_max_qa_iterations")]
    pub max_qa_iterations: u8,
    #[serde(default = "default_qa_timeout_secs")]
    pub qa_timeout_secs: u64,
    #[serde(default)]
    pub auth_strategy: String,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub worktree_branch: Option<String>,
}

fn default_cli() -> String {
    "claude".to_string()
}

fn default_max_qa_iterations() -> u8 {
    DEFAULT_MAX_QA_ITERATIONS
}

fn default_qa_timeout_secs() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub enum SessionTypeInfo {
    Hive { worker_count: u8 },
    Swarm { planner_count: u8 },
    Fusion { variants: Vec<String> },
    Solo { cli: String, model: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct PersistedAgentInfo {
    pub id: String,
    pub role: String,
    pub config: PersistedAgentConfig,
    pub parent_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_trimmed_string")]
    pub commit_sha: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_trimmed_string")]
    pub base_commit_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct PersistedAgentConfig {
    pub cli: String,
    pub model: Option<String>,
    pub flags: Vec<String>,
    pub label: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_trimmed_string")]
    pub name: Option<String>,
    pub description: Option<String>,
    pub role_type: Option<String>,
    pub initial_prompt: Option<String>,
}

#[derive(Debug, Clone)]
struct SessionSyncState {
    modified_at: Option<SystemTime>,
    hash: u64,
}

#[derive(Debug, Clone)]
pub struct SessionRefreshCandidate {
    pub persisted: PersistedSession,
    expected_clean_hash: u64,
    file_modified_at: SystemTime,
}

/// Manages session storage in %APPDATA%/hive-manager
pub struct SessionStorage {
    base_dir: PathBuf,
    artifact_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    session_sync: Mutex<HashMap<String, SessionSyncState>>,
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

        Ok(Self {
            base_dir,
            artifact_locks: Mutex::new(HashMap::new()),
            session_sync: Mutex::new(HashMap::new()),
        })
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

    pub fn user_templates_dir(&self) -> PathBuf {
        self.templates_dir().join("sessions")
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
        fs::create_dir_all(session_dir.join("lessons"))?;
        fs::create_dir_all(session_dir.join("lessons").join("archive"))?;
        // Initialize empty state files
        fs::write(session_dir.join("state").join("workers.md"), "# Available Workers\n\nNo workers yet.\n")?;
        fs::write(session_dir.join("state").join("hierarchy.json"), "[]")?;
        fs::write(session_dir.join("state").join("assignments.json"), "{}")?;

        // Initialize coordination files
        fs::write(session_dir.join("coordination").join("coordination.log"), "")?;
        fs::write(session_dir.join("coordination").join("queen-inbox.md"), "# Queen Inbox\n\nNo messages yet.\n")?;
        self.create_conversation_dir(session_id)?;

        Ok(session_dir)
    }

    /// Create conversation directory and default files for a session.
    pub fn create_conversation_dir(&self, session_id: &str) -> Result<(), StorageError> {
        let conversations_dir = self.session_dir(session_id).join("conversations");
        fs::create_dir_all(&conversations_dir)?;
        for name in ["queen.md", "shared.md"] {
            let path = conversations_dir.join(name);
            if !path.exists() {
                fs::write(path, "")?;
            }
        }
        Ok(())
    }


    /// Save session metadata to disk
    #[allow(dead_code)]
    pub fn save_session(&self, session: &PersistedSession) -> Result<(), StorageError> {
        let session_dir = self.session_dir(&session.id);
        if !session_dir.exists() {
            self.create_session_dir(&session.id)?;
        }

        let session_file = self.session_file_path(&session.id);
        self.atomic_write_json(&session_file, session)?;
        self.mark_session_synced(&session.id, session)?;

        Ok(())
    }

    /// Load session metadata from disk
    pub fn load_session(&self, session_id: &str) -> Result<PersistedSession, StorageError> {
        let session_file = self.session_file_path(session_id);
        if !session_file.exists() {
            return Err(StorageError::SessionNotFound(session_id.to_string()));
        }

        let json = fs::read_to_string(session_file)?;
        let session: PersistedSession = serde_json::from_str(&json)?;

        Ok(session)
    }

    pub fn mark_session_synced(
        &self,
        session_id: &str,
        session: &PersistedSession,
    ) -> Result<(), StorageError> {
        let sync_state = SessionSyncState {
            modified_at: self.session_file_modified_at(session_id)?,
            hash: Self::session_content_hash(session),
        };
        self.session_sync
            .lock()
            .insert(session_id.to_string(), sync_state);
        Ok(())
    }

    pub fn has_newer_session_file(&self, session_id: &str) -> Result<bool, StorageError> {
        let sync_state = {
            let sync = self.session_sync.lock();
            sync.get(session_id).cloned()
        };
        let Some(sync_state) = sync_state else {
            return Ok(false);
        };

        let Some(current_file_mtime) = self.session_file_modified_at(session_id)? else {
            return Ok(false);
        };

        Ok(sync_state.modified_at != Some(current_file_mtime))
    }

    pub fn load_session_if_newer_and_clean(
        &self,
        session_id: &str,
        current_hash: u64,
    ) -> Result<Option<SessionRefreshCandidate>, StorageError> {
        let sync_state = {
            let sync = self.session_sync.lock();
            sync.get(session_id).cloned()
        };
        let Some(sync_state) = sync_state else {
            return Ok(None);
        };

        let current_file_mtime = self.session_file_modified_at(session_id)?;
        let Some(current_file_mtime) = current_file_mtime else {
            return Ok(None);
        };
        if sync_state.modified_at == Some(current_file_mtime) {
            return Ok(None);
        }

        if current_hash != sync_state.hash {
            return Ok(None);
        }

        let persisted = self.load_session(session_id)?;
        Ok(Some(SessionRefreshCandidate {
            persisted,
            expected_clean_hash: sync_state.hash,
            file_modified_at: current_file_mtime,
        }))
    }

    pub fn should_apply_session_refresh(
        &self,
        session_id: &str,
        candidate: &SessionRefreshCandidate,
        current_hash: u64,
    ) -> Result<bool, StorageError> {
        if current_hash != candidate.expected_clean_hash {
            return Ok(false);
        }

        let Some(current_file_mtime) = self.session_file_modified_at(session_id)? else {
            return Ok(false);
        };

        Ok(current_file_mtime == candidate.file_modified_at)
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
                        SessionTypeInfo::Solo { cli, .. } => format!("Solo ({})", cli),
                    };

                    summaries.push(SessionSummary {
                        id: session.id,
                        name: session.name,
                        color: session.color,
                        session_type,
                        project_path: session.project_path,
                        created_at: session.created_at,
                        last_activity_at: session
                            .last_activity_at
                            .unwrap_or(session.created_at),
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
            default_model: "opencode/big-pickle".to_string(),
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
            default_model: "gpt-5.5".to_string(),
            env: None,
        });

        clis.insert("cursor".to_string(), CliConfig {
            command: "wsl".to_string(),
            auto_approve_flag: Some("--force".to_string()),
            model_flag: None,  // Cursor uses global model setting
            default_model: "composer-2".to_string(),
            env: None,
        });

        clis.insert("droid".to_string(), CliConfig {
            command: "droid".to_string(),
            auto_approve_flag: None,  // Interactive mode - no auto-approve flag
            model_flag: None,  // Model selected via /model command in TUI
            default_model: "glm-5.1".to_string(),
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
            cli: "codex".to_string(),
            model: "gpt-5.5".to_string(),
        });
        default_roles.insert("frontend".to_string(), RoleDefaults {
            cli: "gemini".to_string(),
            model: "gemini-2.5-pro".to_string(),
        });
        default_roles.insert("coherence".to_string(), RoleDefaults {
            cli: "codex".to_string(),
            model: "gpt-5.5".to_string(),
        });
        default_roles.insert("simplify".to_string(), RoleDefaults {
            cli: "codex".to_string(),
            model: "gpt-5.5".to_string(),
        });
        default_roles.insert("reviewer".to_string(), RoleDefaults {
            cli: "codex".to_string(),
            model: "gpt-5.5".to_string(),
        });
        default_roles.insert("reviewer-quick".to_string(), RoleDefaults {
            cli: "codex".to_string(),
            model: "gpt-5.5".to_string(),
        });
        default_roles.insert("resolver".to_string(), RoleDefaults {
            cli: "codex".to_string(),
            model: "gpt-5.5".to_string(),
        });
        default_roles.insert("tester".to_string(), RoleDefaults {
            cli: "codex".to_string(),
            model: "gpt-5.5".to_string(),
        });
        default_roles.insert("code-quality".to_string(), RoleDefaults {
            cli: "codex".to_string(),
            model: "gpt-5.5".to_string(),
        });
        default_roles.insert("evaluator".to_string(), RoleDefaults {
            cli: "claude".to_string(),
            model: "opus".to_string(),
        });
        default_roles.insert("qa-worker".to_string(), RoleDefaults {
            cli: "codex".to_string(),
            model: "gpt-5.5".to_string(),
        });
        default_roles.insert("general".to_string(), RoleDefaults {
            cli: "codex".to_string(),
            model: "gpt-5.5".to_string(),
        });

        AppConfig {
            clis,
            default_roles,
            api: ApiConfig {
                enabled: true,
                port: 18800,
            },
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

    /// Append a conversation message to the agent's conversation file.
    /// Uses simple append-mode file I/O (no fs2 locking) to avoid Windows "Access is denied" errors.
    pub async fn append_conversation_message(
        &self,
        session_id: &str,
        agent_id: &str,
        from: &str,
        content: &str,
    ) -> Result<ConversationMessage, StorageError> {
        let conversations_dir = self.session_dir(session_id).join("conversations");
        fs::create_dir_all(&conversations_dir)?;
        let path = self.conversation_file_path(session_id, agent_id);
        let message = ConversationMessage {
            timestamp: Utc::now(),
            from: from.to_string(),
            content: content.to_string(),
        };
        let entry = format!(
            "---\n[{}] from @{}\n{}\n\n",
            message.timestamp.to_rfc3339(),
            message.from,
            message.content
        );

        tokio::task::spawn_blocking(move || -> Result<(), StorageError> {
            let mut file = OpenOptions::new().create(true).append(true).open(path)?;
            file.write_all(entry.as_bytes())?;
            Ok(())
        })
        .await
        .map_err(|e| StorageError::InvalidPath(format!("Join error in append conversation: {}", e)))??;

        Ok(message)
    }

    /// Read conversation messages with optional since filter.
    pub async fn read_conversation(
        &self,
        session_id: &str,
        agent_id: &str,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ConversationMessage>, StorageError> {
        let path = self.conversation_file_path(session_id, agent_id);

        tokio::task::spawn_blocking(move || -> Result<Vec<ConversationMessage>, StorageError> {
            if !path.exists() {
                return Ok(Vec::new());
            }
            let content = fs::read_to_string(&path)?;
            let mut messages = parse_conversation_messages(&content);
            if let Some(since_ts) = since {
                messages.retain(|m| m.timestamp > since_ts);
            }
            Ok(messages)
        })
        .await
        .map_err(|e| StorageError::InvalidPath(format!("Join error in read conversation: {}", e)))?
    }

    fn conversation_file_path(&self, session_id: &str, agent_id: &str) -> PathBuf {
        self.session_dir(session_id)
            .join("conversations")
            .join(format!("{}.md", agent_id))
    }

    fn artifact_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("artifacts")
    }

    fn artifact_file_path(&self, session_id: &str, cell_id: &str) -> PathBuf {
        self.artifact_dir(session_id).join(format!("{}.json", cell_id))
    }

    fn artifact_lock(&self, session_id: &str, cell_id: &str) -> Arc<Mutex<()>> {
        let key = format!("{session_id}:{cell_id}");
        let mut locks = self.artifact_locks.lock();
        locks
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn resolver_output_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("resolver_output.json")
    }

    fn user_template_path(&self, template_id: &str) -> PathBuf {
        self.user_templates_dir().join(format!("{}.json", template_id))
    }

    fn ai_docs_dir(project_path: &Path) -> PathBuf {
        project_path.join(".ai-docs")
    }

    /// Get the session-scoped lessons directory
    /// Stores learnings and project DNA in .hive-manager/{session_id}/lessons/
    fn session_lessons_dir(&self, session_id: &str) -> PathBuf {
        // For per-session lessons, store in %APPDATA%/hive-manager/sessions/{session_id}/lessons/
        // This allows multi-project sessions without conflicts
        self.session_dir(session_id).join("lessons")
    }

    /// Append a learning to the .ai-docs/learnings.jsonl file (project-scoped, legacy)
    /// DEPRECATED: Use append_learning_session for new code
    pub fn append_learning(&self, project_path: &Path, learning: &Learning) -> Result<(), StorageError> {
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
    pub fn append_learning_session(&self, session_id: &str, learning: &Learning) -> Result<(), StorageError> {
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
    pub fn delete_learning_session(&self, session_id: &str, learning_id: &str) -> Result<bool, StorageError> {
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
        let mut temp = tempfile::NamedTempFile::new_in(&lessons_dir)
            .map_err(|e| StorageError::Io(e))?;
        for line in &remaining_lines {
            writeln!(temp, "{}", line)
                .map_err(|e| StorageError::Io(e))?;
        }
        temp.persist(&learnings_file)
            .map_err(|e| StorageError::Io(e.error))?;

        Ok(true)
    }

    /// Read all learnings from the session-scoped lessons directory
    /// Reads from .hive-manager/{session_id}/lessons/learnings.jsonl
    pub fn read_learnings_session(&self, session_id: &str) -> Result<Vec<Learning>, StorageError> {
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
    pub fn read_project_dna_session(&self, session_id: &str) -> Result<String, StorageError> {
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
    #[allow(dead_code)]
    pub fn save_project_dna_session(&self, session_id: &str, content: &str) -> Result<(), StorageError> {
        let lessons_dir = self.session_lessons_dir(session_id);
        fs::create_dir_all(&lessons_dir)?;
        let project_dna_file = lessons_dir.join("project-dna.md");
        fs::write(project_dna_file, content)?;
        Ok(())
    }

    pub fn save_artifact(
        &self,
        session_id: &str,
        cell_id: &str,
        artifact: &ArtifactBundle,
    ) -> Result<(), StorageError> {
        let artifact_dir = self.artifact_dir(session_id);
        fs::create_dir_all(&artifact_dir)?;

        if cell_id == PRIMARY_CELL_ID {
            let lock = self.artifact_lock(session_id, cell_id);
            let _guard = lock.lock();
            self.atomic_write_json(&self.artifact_file_path(session_id, cell_id), artifact)
        } else {
            self.atomic_write_json(&self.artifact_file_path(session_id, cell_id), artifact)
        }
    }

    pub fn load_artifact(
        &self,
        session_id: &str,
        cell_id: &str,
    ) -> Result<Option<ArtifactBundle>, StorageError> {
        let path = self.artifact_file_path(session_id, cell_id);
        self.read_optional_json(&path)
    }

    pub fn atomic_update_artifact<F>(
        &self,
        session_id: &str,
        cell_id: &str,
        update: F,
    ) -> Result<ArtifactBundle, StorageError>
    where
        F: FnOnce(Option<ArtifactBundle>) -> ArtifactBundle,
    {
        let artifact_dir = self.artifact_dir(session_id);
        fs::create_dir_all(&artifact_dir)?;

        let lock = self.artifact_lock(session_id, cell_id);
        let _guard = lock.lock();

        let path = self.artifact_file_path(session_id, cell_id);
        let current = self.read_optional_json(&path)?;
        let updated = update(current);
        self.atomic_write_json(&path, &updated)?;
        Ok(updated)
    }

    pub fn save_resolver_output(
        &self,
        session_id: &str,
        output: &ResolverOutput,
    ) -> Result<(), StorageError> {
        let session_dir = self.session_dir(session_id);
        fs::create_dir_all(&session_dir)?;
        self.atomic_write_json(&self.resolver_output_path(session_id), output)
    }

    pub fn load_resolver_output(
        &self,
        session_id: &str,
    ) -> Result<Option<ResolverOutput>, StorageError> {
        self.read_optional_json(&self.resolver_output_path(session_id))
    }

    pub fn save_user_template(&self, template: &SessionTemplate) -> Result<(), StorageError> {
        let templates_dir = self.user_templates_dir();
        fs::create_dir_all(&templates_dir)?;
        self.atomic_write_json(&self.user_template_path(&template.id), template)
    }

    pub fn load_user_template(
        &self,
        template_id: &str,
    ) -> Result<Option<SessionTemplate>, StorageError> {
        self.read_optional_json(&self.user_template_path(template_id))
    }

    pub fn list_user_templates(&self) -> Result<Vec<SessionTemplate>, StorageError> {
        let templates_dir = self.user_templates_dir();
        if !templates_dir.exists() {
            return Ok(Vec::new());
        }

        let mut templates = Vec::new();
        for entry in fs::read_dir(templates_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }

            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let template: SessionTemplate = serde_json::from_str(&fs::read_to_string(entry.path())?)?;
            templates.push(template);
        }

        templates.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(templates)
    }

    pub fn delete_user_template(&self, template_id: &str) -> Result<bool, StorageError> {
        let path = self.user_template_path(template_id);
        if !path.exists() {
            return Ok(false);
        }

        fs::remove_file(path)?;
        Ok(true)
    }

    pub fn read_latest_conversation_message(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Option<String>, StorageError> {
        let path = self.conversation_file_path(session_id, agent_id);
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(path)?;
        Ok(parse_conversation_messages(&content)
            .into_iter()
            .last()
            .map(|message| message.content))
    }

    fn atomic_write_json<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), StorageError> {
        let parent = path.parent().ok_or_else(|| {
            StorageError::InvalidPath(format!("No parent directory for {}", path.display()))
        })?;
        fs::create_dir_all(parent)?;

        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(StorageError::Io)?;
        serde_json::to_writer_pretty(&mut temp, value)?;
        temp.persist(path).map_err(|e| StorageError::Io(e.error))?;
        Ok(())
    }

    fn read_optional_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &Path,
    ) -> Result<Option<T>, StorageError> {
        if !path.exists() {
            return Ok(None);
        }

        let value = serde_json::from_str(&fs::read_to_string(path)?)?;
        Ok(Some(value))
    }

    fn session_file_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("session.json")
    }

    fn session_file_modified_at(
        &self,
        session_id: &str,
    ) -> Result<Option<SystemTime>, StorageError> {
        let path = self.session_file_path(session_id);
        if !path.exists() {
            return Ok(None);
        }

        Ok(fs::metadata(path)?.modified().ok())
    }

    pub fn session_content_hash(session: &PersistedSession) -> u64 {
        let mut hasher = DefaultHasher::new();
        session.hash(&mut hasher);
        hasher.finish()
    }
}

fn parse_conversation_messages(content: &str) -> Vec<ConversationMessage> {
    // Entry format:
    // ---
    // [timestamp] from @sender
    // message body
    // (blank line)
    let mut messages = Vec::new();
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return messages;
    }

    let normalized = trimmed.strip_prefix("---\n").unwrap_or(trimmed);
    let chunks: Vec<&str> = normalized.split("\n---\n").collect();
    for raw in chunks {
        let chunk = raw.trim();
        if chunk.is_empty() {
            continue;
        }
        let mut lines = chunk.lines();
        let header = match lines.next() {
            Some(h) => h.trim(),
            None => continue,
        };
        let caps = match regex::Regex::new(r"^\[([^\]]+)\] from @([A-Za-z0-9\-]+)$")
            .ok()
            .and_then(|re| re.captures(header))
        {
            Some(c) => c,
            None => continue,
        };
        let timestamp = match DateTime::parse_from_rfc3339(&caps[1]) {
            Ok(ts) => ts.with_timezone(&Utc),
            Err(_) => continue,
        };
        let from = caps[2].to_string();
        let message_body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
        messages.push(ConversationMessage {
            timestamp,
            from,
            content: message_body,
        });
    }
    messages
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
            enabled: true,   // Enabled by default for Queen to spawn workers
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
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    fn create_test_storage() -> (SessionStorage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(temp_dir.path().to_path_buf()).unwrap();
        (storage, temp_dir)
    }

    #[test]
    fn test_default_role_models_match_frontend_defaults() {
        let config = SessionStorage::default_config();

        for role in [
            "backend",
            "coherence",
            "simplify",
            "reviewer",
            "reviewer-quick",
            "resolver",
            "tester",
            "code-quality",
            "qa-worker",
            "general",
        ] {
            let defaults = config.default_roles.get(role).unwrap();
            assert_eq!(defaults.cli, "codex", "role {role} should default to codex");
            assert_eq!(defaults.model, "gpt-5.5", "role {role} should default to gpt-5.5");
        }

        let frontend = config.default_roles.get("frontend").unwrap();
        assert_eq!(frontend.cli, "gemini");
        assert_eq!(frontend.model, "gemini-2.5-pro");

        let evaluator = config.default_roles.get("evaluator").unwrap();
        assert_eq!(evaluator.cli, "claude");
        assert_eq!(evaluator.model, "opus");
    }

    fn sample_persisted_session(session_id: &str) -> PersistedSession {
        PersistedSession {
            id: session_id.to_string(),
            name: Some("Test Session".to_string()),
            color: None,
            session_type: SessionTypeInfo::Hive { worker_count: 1 },
            project_path: "D:/tmp/project".to_string(),
            created_at: Utc::now(),
            last_activity_at: Some(Utc::now()),
            agents: vec![PersistedAgentInfo {
                id: format!("{session_id}-worker-1"),
                role: "Worker(1)".to_string(),
                config: PersistedAgentConfig {
                    cli: "codex".to_string(),
                    model: None,
                    flags: vec![],
                    label: None,
                    name: None,
                    description: None,
                    role_type: None,
                    initial_prompt: None,
                },
                parent_id: Some(format!("{session_id}-queen")),
                commit_sha: None,
                base_commit_sha: None,
            }],
            state: "Running".to_string(),
            default_cli: "codex".to_string(),
            default_model: None,
            qa_workers: vec![],
            max_qa_iterations: default_max_qa_iterations(),
            qa_timeout_secs: default_qa_timeout_secs(),
            auth_strategy: String::new(),
            worktree_path: None,
            worktree_branch: None,
        }
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
    fn test_persisted_agent_blank_commit_sha_deserializes_to_none() {
        let json = r#"{
            "id": "agent-1",
            "role": "Worker(1)",
            "config": {
                "cli": "codex",
                "model": null,
                "flags": [],
                "label": null,
                "name": null,
                "description": null,
                "role_type": null,
                "initial_prompt": null
            },
            "parent_id": null,
            "commit_sha": "   "
        }"#;

        let agent: PersistedAgentInfo = serde_json::from_str(json).unwrap();
        assert_eq!(agent.commit_sha, None);
    }

    #[test]
    fn test_load_session_if_newer_and_clean_uses_cached_hash() {
        let (storage, _temp_dir) = create_test_storage();
        let session = sample_persisted_session("refresh-hash-session");
        storage.save_session(&session).unwrap();

        let clean_hash = SessionStorage::session_content_hash(&session);

        let mut updated = session.clone();
        updated.state = "QaPassed".to_string();
        let baseline_mtime = storage
            .session_file_modified_at(&session.id)
            .unwrap();
        let session_file = storage.session_file_path(&session.id);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            storage.atomic_write_json(&session_file, &updated).unwrap();
            let now_mtime = storage
                .session_file_modified_at(&session.id)
                .unwrap();
            if now_mtime != baseline_mtime {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("session.json mtime did not change within timeout");
            }
            thread::sleep(Duration::from_millis(50));
        }

        assert!(storage.has_newer_session_file(&session.id).unwrap());

        let candidate = storage
            .load_session_if_newer_and_clean(&session.id, clean_hash)
            .unwrap()
            .expect("refresh candidate");
        assert_eq!(candidate.persisted.state, "QaPassed");
        assert!(storage
            .should_apply_session_refresh(&session.id, &candidate, clean_hash)
            .unwrap());

        let mut dirty_in_memory = session.clone();
        dirty_in_memory.state = "Dirty".to_string();
        let dirty_hash = SessionStorage::session_content_hash(&dirty_in_memory);
        assert!(storage
            .load_session_if_newer_and_clean(&session.id, dirty_hash)
            .unwrap()
            .is_none());
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
        storage.append_learning_session(session_id, &learning).unwrap();

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

        storage.append_learning_session(session_id, &learning1).unwrap();
        storage.append_learning_session(session_id, &learning2).unwrap();
        storage.append_learning_session(session_id, &learning3).unwrap();

        // Delete the middle one
        let deleted = storage.delete_learning_session(session_id, "learning-2").unwrap();
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

        storage.append_learning_session(session_id, &learning).unwrap();

        // Try to delete non-existent learning
        let deleted = storage.delete_learning_session(session_id, "nonexistent-id").unwrap();
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

        let learnings_file = storage.session_lessons_dir(session_id).join("learnings.jsonl");
        
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

    #[test]
    fn test_primary_cell_save_artifact_waits_for_existing_lock() {
        let (storage, _temp_dir) = create_test_storage();
        let storage = Arc::new(storage);
        let session_id = "test-session-primary-artifact-lock";

        storage.create_session_dir(session_id).unwrap();

        let artifact = ArtifactBundle {
            summary: Some("summary".to_string()),
            changed_files: vec!["src/main.rs".to_string()],
            commits: vec!["abc123".to_string()],
            branch: "feature/primary-lock".to_string(),
            test_results: None,
            diff_summary: None,
            unresolved_issues: vec![],
            confidence: Some(0.9),
            recommended_next_step: None,
        };

        let lock = storage.artifact_lock(session_id, PRIMARY_CELL_ID);
        let guard = lock.lock();

        let (done_tx, done_rx) = mpsc::channel();
        let storage_clone = Arc::clone(&storage);
        let artifact_clone = artifact.clone();

        let save_thread = thread::spawn(move || {
            storage_clone
                .save_artifact(session_id, PRIMARY_CELL_ID, &artifact_clone)
                .unwrap();
            done_tx.send(()).unwrap();
        });

        assert!(
            done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "save_artifact should wait while the primary cell lock is held"
        );

        drop(guard);

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("save_artifact should complete after the primary cell lock is released");
        save_thread.join().unwrap();

        let saved = storage
            .load_artifact(session_id, PRIMARY_CELL_ID)
            .unwrap()
            .expect("artifact should be persisted");
        assert_eq!(saved.branch, artifact.branch);
    }
}
