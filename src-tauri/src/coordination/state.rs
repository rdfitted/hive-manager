use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::pty::WorkerRole;

#[derive(Debug, Error)]
pub enum StateError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
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

    /// Ensure state directory exists
    fn ensure_state_dir(&self) -> Result<(), StateError> {
        fs::create_dir_all(self.state_dir())?;
        Ok(())
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
            nodes.into_iter().filter(|n| n.role != "Queen").map(|n| {
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
                }
            }).collect()
        })
    }

    /// Update the hierarchy.json file
    pub fn update_hierarchy(&self, hierarchy: &[HierarchyNode]) -> Result<(), StateError> {
        self.ensure_state_dir()?;

        let hierarchy_path = self.state_dir().join("hierarchy.json");
        let json = serde_json::to_string_pretty(hierarchy)?;
        fs::write(hierarchy_path, json)?;

        Ok(())
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
}
