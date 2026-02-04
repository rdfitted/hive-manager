use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::pty::{AgentRole, AgentStatus, PtyManager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionType {
    Hive { worker_count: u8 },
    Swarm { planner_count: u8 },
    Fusion { variants: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionState {
    Starting,
    Running,
    Paused,
    Completed,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub role: AgentRole,
    pub status: AgentStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub session_type: SessionType,
    pub project_path: PathBuf,
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    pub agents: Vec<AgentInfo>,
}

#[derive(Clone, Serialize)]
pub struct SessionUpdate {
    pub session: Session,
}

pub struct SessionController {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    pty_manager: Arc<RwLock<PtyManager>>,
    app_handle: Option<AppHandle>,
}

// Explicitly implement Send + Sync
unsafe impl Send for SessionController {}
unsafe impl Sync for SessionController {}

impl SessionController {
    pub fn new(pty_manager: Arc<RwLock<PtyManager>>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            pty_manager,
            app_handle: None,
        }
    }

    pub fn set_app_handle(&mut self, handle: AppHandle) {
        self.app_handle = Some(handle.clone());
        let mut pty_manager = self.pty_manager.write();
        pty_manager.set_app_handle(handle);
    }

    pub fn launch_hive(
        &self,
        project_path: PathBuf,
        worker_count: u8,
        command: &str,
        prompt: Option<String>,
    ) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let mut agents = Vec::new();
        let prompt_str = prompt.unwrap_or_default();
        let cwd = project_path.to_str().unwrap_or(".");

        // Parse command - support "command arg1 arg2" format
        let parts: Vec<&str> = command.split_whitespace().collect();
        let (cmd, base_args) = if parts.is_empty() {
            ("cmd.exe", vec![])
        } else {
            (parts[0], parts[1..].to_vec())
        };

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let mut queen_args = base_args.clone();

            // Add prompt arg if provided and command is claude
            if cmd == "claude" && !prompt_str.is_empty() {
                queen_args.push("-p");
                // Note: prompt would need special handling for spaces
            }

            tracing::info!("Launching Queen agent: {} {:?} in {:?}", cmd, queen_args, project_path);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    cmd,
                    &queen_args.iter().map(|s| *s).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| {
                    let err_msg = format!("Failed to spawn Queen: {}", e);
                    tracing::error!("{}", err_msg);
                    err_msg
                })?;

            agents.push(AgentInfo {
                id: queen_id,
                role: AgentRole::Queen,
                status: AgentStatus::Running,
            });

            // Create Worker agents
            for i in 1..=worker_count {
                let worker_id = format!("{}-worker-{}", session_id, i);
                let worker_args = base_args.clone();

                tracing::info!("Launching Worker {} agent: {} {:?} in {:?}", i, cmd, worker_args, project_path);

                pty_manager
                    .create_session(
                        worker_id.clone(),
                        AgentRole::Worker { index: i, parent: None },
                        cmd,
                        &worker_args.iter().map(|s| *s).collect::<Vec<_>>(),
                        Some(cwd),
                        120,
                        30,
                    )
                    .map_err(|e| {
                        let err_msg = format!("Failed to spawn Worker {}: {}", i, e);
                        tracing::error!("{}", err_msg);
                        err_msg
                    })?;

                agents.push(AgentInfo {
                    id: worker_id,
                    role: AgentRole::Worker { index: i, parent: None },
                    status: AgentStatus::Running,
                });
            }
        }

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Hive { worker_count },
            project_path,
            state: SessionState::Running,
            created_at: Utc::now(),
            agents,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        Ok(session)
    }

    pub fn get_session(&self, id: &str) -> Option<Session> {
        let sessions = self.sessions.read();
        sessions.get(id).cloned()
    }

    pub fn list_sessions(&self) -> Vec<Session> {
        let sessions = self.sessions.read();
        sessions.values().cloned().collect()
    }

    pub fn stop_session(&self, id: &str) -> Result<(), String> {
        let session = {
            let sessions = self.sessions.read();
            sessions.get(id).cloned()
        };

        if let Some(session) = session {
            let pty_manager = self.pty_manager.read();
            for agent in &session.agents {
                let _ = pty_manager.kill(&agent.id);
            }

            {
                let mut sessions = self.sessions.write();
                if let Some(s) = sessions.get_mut(id) {
                    s.state = SessionState::Completed;
                }
            }

            if let Some(ref app_handle) = self.app_handle {
                let sessions = self.sessions.read();
                if let Some(session) = sessions.get(id) {
                    let _ = app_handle.emit("session-update", SessionUpdate {
                        session: session.clone(),
                    });
                }
            }

            Ok(())
        } else {
            Err(format!("Session not found: {}", id))
        }
    }

    pub fn stop_agent(&self, session_id: &str, agent_id: &str) -> Result<(), String> {
        let pty_manager = self.pty_manager.read();
        pty_manager.kill(agent_id).map_err(|e| e.to_string())?;

        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                if let Some(agent) = session.agents.iter_mut().find(|a| a.id == agent_id) {
                    agent.status = AgentStatus::Completed;
                }
            }
        }

        Ok(())
    }

}

impl Default for SessionController {
    fn default() -> Self {
        Self::new(Arc::new(RwLock::new(PtyManager::new())))
    }
}
