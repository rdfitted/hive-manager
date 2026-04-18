use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::pty::WorkerRole;

use super::{parse_sprint_contract, SprintContract};

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum StateError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("contract parse error: {0}")]
    ContractParse(String),
    #[error("contracts are immutable once QA begins for session state: {0}")]
    ContractLocked(String),
    #[allow(dead_code)]
    #[error("Session not found: {0}")]
    SessionNotFound(String),
}

/// Information about a worker for state tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerStateInfo {
    pub id: String,
    pub role: WorkerRole,
    pub cli: String,
    pub status: String,
    pub current_task: Option<String>,
    pub last_update: DateTime<Utc>,
    #[serde(default)]
    pub last_heartbeat: Option<DateTime<Utc>>,
}

/// Agent hierarchy node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchyNode {
    pub id: String,
    pub role: String,
    pub parent_id: Option<String>,
    pub children: Vec<String>,
}

/// Task assignment record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAssignment {
    pub worker_id: String,
    pub task: String,
    pub assigned_at: DateTime<Utc>,
    pub status: AssignmentStatus,
    pub plan_task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerMessageRecord {
    pub kind: String,
    pub from: String,
    pub to: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub commit_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AssignmentStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// Manages state files for a session
pub struct StateManager {
    session_path: PathBuf,
}

impl StateManager {
    /// Create a new state manager for a session
    pub fn new(session_path: PathBuf) -> Self {
        Self { session_path }
    }

    /// Get path to state directory
    fn state_dir(&self) -> PathBuf {
        self.session_path.join("state")
    }

    fn peer_dir(&self) -> PathBuf {
        self.session_path.join("peer")
    }

    fn contracts_dir(&self) -> PathBuf {
        self.session_path.join("contracts")
    }

    /// Ensure state directory exists
    fn ensure_state_dir(&self) -> Result<(), StateError> {
        fs::create_dir_all(self.state_dir())?;
        Ok(())
    }

    fn ensure_peer_dir(&self) -> Result<(), StateError> {
        fs::create_dir_all(self.peer_dir())?;
        Ok(())
    }

    fn ensure_contracts_dir(&self) -> Result<(), StateError> {
        fs::create_dir_all(self.contracts_dir())?;
        Ok(())
    }

    fn write_atomic_text(&self, target: PathBuf, content: &str) -> Result<(), StateError> {
        let parent = target
            .parent()
            .ok_or_else(|| StateError::Io(std::io::Error::other("target has no parent directory")))?;
        fs::create_dir_all(parent)?;

        let mut temp = NamedTempFile::new_in(parent)?;
        use std::io::Write;
        temp.write_all(content.as_bytes())?;
        temp.persist(target)
            .map_err(|err| StateError::Io(err.error))?;
        Ok(())
    }

    fn write_peer_record(
        &self,
        file_name: &str,
        record: &PeerMessageRecord,
    ) -> Result<(), StateError> {
        self.ensure_peer_dir()?;

        let peer_dir = self.peer_dir();
        let target = peer_dir.join(file_name);
        let json = serde_json::to_string_pretty(record)?;
        self.write_atomic_text(target, &json)
    }

    /// Update the workers.md file (Queen reads this)
    pub fn update_workers_file(&self, workers: &[WorkerStateInfo]) -> Result<(), StateError> {
        self.ensure_state_dir()?;

        let mut content = String::from("# Available Workers\n\n");

        if workers.is_empty() {
            content.push_str("No workers assigned yet.\n");
        } else {
            // Table header
            content.push_str("## Active Workers\n\n");
            content.push_str("| ID | Role | CLI | Status | Current Task |\n");
            content.push_str("|----|------|-----|--------|---------------|\n");

            for worker in workers {
                let task = worker.current_task.as_deref().unwrap_or("-");
                content.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    worker.id, worker.role.label, worker.cli, worker.status, task
                ));
            }

            // Worker capabilities section
            content.push_str("\n## Worker Capabilities\n\n");
            for worker in workers {
                content.push_str(&format!("### {} ({})\n", worker.id, worker.role.label));
                content.push_str(&format!("- CLI: {}\n", worker.cli));
                content.push_str(&format!("- Specialization: {}\n", self.get_role_description(&worker.role)));
                content.push_str("\n");
            }

            // Communication instructions
            content.push_str("## Communication\n\n");
            content.push_str("To assign a task to a worker, the Queen should:\n");
            content.push_str("1. Update this file with the assignment\n");
            content.push_str("2. The system will inject the task into the worker's terminal\n");
        }

        let workers_path = self.state_dir().join("workers.md");
        fs::write(workers_path, content)?;

        Ok(())
    }

    /// Get role description for capabilities section
    fn get_role_description(&self, role: &WorkerRole) -> &str {
        match role.role_type.to_lowercase().as_str() {
            "backend" => "Server-side logic, APIs, databases",
            "frontend" => "UI components, state management, styling",
            "coherence" => "Code consistency, API contract verification",
            "simplify" => "Code simplification, refactoring",
            _ => "General development tasks",
        }
    }

    /// Read workers from the workers.md file
    pub fn read_workers_file(&self) -> Result<Vec<WorkerStateInfo>, StateError> {
        let workers_path = self.state_dir().join("workers.md");
        if !workers_path.exists() {
            return Ok(vec![]);
        }

        // For now, we read from hierarchy.json instead since that's more reliable
        // workers.md is mainly for the Queen to read
        self.read_hierarchy().map(|nodes| {
            nodes.into_iter().filter(|n| n.role != "Queen" && n.role != "Evaluator" && !n.role.starts_with("QaWorker-")).map(|n| {
                WorkerStateInfo {
                    id: n.id,
                    role: WorkerRole {
                        role_type: n.role.clone(),
                        label: n.role,
                        default_cli: "claude".to_string(),
                        prompt_template: None,
                    },
                    cli: "claude".to_string(),
                    status: "Running".to_string(),
                    current_task: None,
                    last_update: Utc::now(),
                    last_heartbeat: None,
                }
            }).collect()
        })
    }

    /// Update the hierarchy.json file
    pub fn update_hierarchy(&self, hierarchy: &[HierarchyNode]) -> Result<(), StateError> {
        self.ensure_state_dir()?;

        let hierarchy_path = self.state_dir().join("hierarchy.json");
        let normalized: Vec<HierarchyNode> = hierarchy
            .iter()
            .cloned()
            .map(|mut node| {
                if node.role == "Evaluator" {
                    node.parent_id = None;
                }
                node
            })
            .collect();
        let json = serde_json::to_string_pretty(&normalized)?;
        fs::write(hierarchy_path, json)?;

        Ok(())
    }

    pub fn write_milestone_ready(
        &self,
        from: &str,
        to: &str,
        content: &str,
    ) -> Result<(), StateError> {
        self.write_peer_record(
            "milestone-ready.json",
            &PeerMessageRecord {
                kind: "milestone-ready".to_string(),
                from: from.to_string(),
                to: to.to_string(),
                content: content.to_string(),
                timestamp: Utc::now(),
                commit_sha: None,
            },
        )
    }

    pub fn write_qa_verdict(
        &self,
        from: &str,
        to: &str,
        content: &str,
        commit_sha: Option<&str>,
    ) -> Result<(), StateError> {
        self.write_peer_record(
            "qa-verdict.json",
            &PeerMessageRecord {
                kind: "qa-verdict".to_string(),
                from: from.to_string(),
                to: to.to_string(),
                content: content.to_string(),
                timestamp: Utc::now(),
                commit_sha: commit_sha.map(str::to_string),
            },
        )
    }

    pub fn write_evaluator_feedback(
        &self,
        from: &str,
        to: &str,
        content: &str,
    ) -> Result<(), StateError> {
        self.write_peer_record(
            "evaluator-feedback.json",
            &PeerMessageRecord {
                kind: "evaluator-feedback".to_string(),
                from: from.to_string(),
                to: to.to_string(),
                content: content.to_string(),
                timestamp: Utc::now(),
                commit_sha: None,
            },
        )
    }

    /// Read the hierarchy from file
    pub fn read_hierarchy(&self) -> Result<Vec<HierarchyNode>, StateError> {
        let hierarchy_path = self.state_dir().join("hierarchy.json");
        if !hierarchy_path.exists() {
            return Ok(vec![]);
        }

        let json = fs::read_to_string(hierarchy_path)?;
        let hierarchy: Vec<HierarchyNode> = serde_json::from_str(&json)?;

        Ok(hierarchy)
    }

    /// Record a task assignment
    pub fn record_assignment(
        &self,
        worker_id: &str,
        task: &str,
        plan_task_id: Option<String>,
    ) -> Result<(), StateError> {
        self.ensure_state_dir()?;

        let assignments_path = self.state_dir().join("assignments.json");
        let mut assignments: HashMap<String, TaskAssignment> = if assignments_path.exists() {
            let json = fs::read_to_string(&assignments_path)?;
            serde_json::from_str(&json)?
        } else {
            HashMap::new()
        };

        assignments.insert(worker_id.to_string(), TaskAssignment {
            worker_id: worker_id.to_string(),
            task: task.to_string(),
            assigned_at: Utc::now(),
            status: AssignmentStatus::Pending,
            plan_task_id,
        });

        let json = serde_json::to_string_pretty(&assignments)?;
        fs::write(assignments_path, json)?;

        Ok(())
    }

    /// Update assignment status
    #[allow(dead_code)]
    pub fn update_assignment_status(
        &self,
        worker_id: &str,
        status: AssignmentStatus,
    ) -> Result<(), StateError> {
        self.ensure_state_dir()?;

        let assignments_path = self.state_dir().join("assignments.json");
        if !assignments_path.exists() {
            return Ok(());
        }

        let json = fs::read_to_string(&assignments_path)?;
        let mut assignments: HashMap<String, TaskAssignment> = serde_json::from_str(&json)?;

        if let Some(assignment) = assignments.get_mut(worker_id) {
            assignment.status = status;
        }

        let json = serde_json::to_string_pretty(&assignments)?;
        fs::write(assignments_path, json)?;

        Ok(())
    }

    /// Get all assignments
    #[allow(dead_code)]
    pub fn get_assignments(&self) -> Result<HashMap<String, TaskAssignment>, StateError> {
        let assignments_path = self.state_dir().join("assignments.json");
        if !assignments_path.exists() {
            return Ok(HashMap::new());
        }

        let json = fs::read_to_string(assignments_path)?;
        let assignments: HashMap<String, TaskAssignment> = serde_json::from_str(&json)?;

        Ok(assignments)
    }

    /// Get assignment for a specific worker
    #[allow(dead_code)]
    pub fn get_worker_assignment(&self, worker_id: &str) -> Result<Option<TaskAssignment>, StateError> {
        let assignments = self.get_assignments()?;
        Ok(assignments.get(worker_id).cloned())
    }

    #[allow(dead_code)]
    pub fn write_contract(
        &self,
        milestone_index: u8,
        markdown: &str,
        session_state: &str,
        qa_locked: bool,
    ) -> Result<SprintContract, StateError> {
        if qa_locked {
            return Err(StateError::ContractLocked(session_state.to_string()));
        }

        self.ensure_contracts_dir()?;

        let contract = parse_sprint_contract(markdown)
            .map_err(|err| StateError::ContractParse(err.to_string()))?;
        let target = self
            .contracts_dir()
            .join(format!("milestone-{}.md", milestone_index));
        self.write_atomic_text(target, markdown)?;
        Ok(contract)
    }

    #[allow(dead_code)]
    pub fn read_contract(&self, milestone_index: u8) -> Result<Option<SprintContract>, StateError> {
        let path = self
            .contracts_dir()
            .join(format!("milestone-{}.md", milestone_index));
        if !path.exists() {
            return Ok(None);
        }

        let markdown = fs::read_to_string(path)?;
        let contract = parse_sprint_contract(&markdown)
            .map_err(|err| StateError::ContractParse(err.to_string()))?;
        Ok(Some(contract))
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn peer_writes_are_atomic_and_overwrite_latest_message() {
        let temp = TempDir::new().unwrap();
        let manager = StateManager::new(temp.path().to_path_buf());

        manager
            .write_milestone_ready("queen", "evaluator", "Milestone A is ready")
            .unwrap();
        manager
            .write_milestone_ready("queen", "evaluator", "Milestone B is ready")
            .unwrap();

        let path = temp.path().join("peer").join("milestone-ready.json");
        let record: PeerMessageRecord = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();

        assert_eq!(record.kind, "milestone-ready");
        assert_eq!(record.content, "Milestone B is ready");
        assert!(temp.path().join("peer").read_dir().unwrap().all(|entry| {
            !entry.unwrap().file_name().to_string_lossy().ends_with(".tmp")
        }));
    }

    #[test]
    fn contract_round_trip_preserves_numbered_criteria() {
        let temp = TempDir::new().unwrap();
        let manager = StateManager::new(temp.path().to_path_buf());
        let markdown = r#"# Sprint Contract: Dashboard polish

## Acceptance Criteria
1. [FUNC] Dashboard loads with current account data
2. [A11Y] Keyboard navigation reaches every control

## Pass Threshold
- All FUNC criteria must PASS
- Scored criteria average >= 7/10
"#;

        let written = manager
            .write_contract(2, markdown, "Running", false)
            .unwrap();
        let read_back = manager.read_contract(2).unwrap().unwrap();

        assert_eq!(written, read_back);
        assert_eq!(read_back.criterion(1).unwrap().description, "Dashboard loads with current account data");
    }

    #[test]
    fn contract_writes_fail_once_qa_is_locked() {
        let temp = TempDir::new().unwrap();
        let manager = StateManager::new(temp.path().to_path_buf());
        let markdown = r#"# Sprint Contract: Locked

## Acceptance Criteria
1. [FUNC] Something passes

## Pass Threshold
- All FUNC criteria must PASS
"#;

        let err = manager
            .write_contract(1, markdown, "QaInProgress", true)
            .unwrap_err();

        assert!(matches!(err, StateError::ContractLocked(state) if state == "QaInProgress"));
    }
}
