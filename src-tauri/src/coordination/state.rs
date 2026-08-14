use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::orchestrator::org_graph::definitions::role_prompt_template;
use crate::orchestrator::org_graph::ownership::{
    derive_path_ownership, LivePrincipal, OrchestratorWriteAttempt, OrchestratorWriteOutcome,
    OwnershipSessionState,
};
use crate::orchestrator::work_graph::divergence::DivergenceSummary;
use crate::orchestrator::work_graph::review::ReviewExpansionSidecar;
use crate::orchestrator::work_graph::runtime::GraphCompositionState;
use crate::orchestrator::work_graph::WorkGraph;
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

/// Durable evidence that a lifecycle transition refreshed the graph artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkGraphLifecycleSnapshot {
    pub lifecycle_stage: String,
    pub emitted_at: DateTime<Utc>,
    pub node_count: usize,
    pub edge_count: usize,
    pub artifact: String,
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
                        label: n.role.clone(),
                        default_cli: "claude".to_string(),
                        prompt_template: Some(role_prompt_template(&n.role)),
                        resolved_definition: None,
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

    pub async fn write_qa_verdict_async(
        &self,
        from: &str,
        to: &str,
        content: &str,
        commit_sha: Option<&str>,
    ) -> Result<(), StateError> {
        let session_path = self.session_path.clone();
        let from = from.to_string();
        let to = to.to_string();
        let content = content.to_string();
        let commit_sha = commit_sha.map(str::to_string);

        tokio::task::spawn_blocking(move || {
            StateManager::new(session_path)
                .write_qa_verdict(&from, &to, &content, commit_sha.as_deref())
        })
        .await
        .map_err(|err| StateError::Io(std::io::Error::other(format!(
            "QA verdict write task failed: {err}"
        ))))?
    }

    /// Write the Prince's remediation verdict (peer/prince-verdict.json). The Queen
    /// polls this file and only pushes the PR once the Prince has self-certified.
    pub fn write_prince_verdict(
        &self,
        from: &str,
        to: &str,
        content: &str,
        commit_sha: Option<&str>,
    ) -> Result<(), StateError> {
        self.write_peer_record(
            "prince-verdict.json",
            &PeerMessageRecord {
                kind: "prince-verdict".to_string(),
                from: from.to_string(),
                to: to.to_string(),
                content: content.to_string(),
                timestamp: Utc::now(),
                commit_sha: commit_sha.map(str::to_string),
            },
        )
    }

    pub async fn write_prince_verdict_async(
        &self,
        from: &str,
        to: &str,
        content: &str,
        commit_sha: Option<&str>,
    ) -> Result<(), StateError> {
        let session_path = self.session_path.clone();
        let from = from.to_string();
        let to = to.to_string();
        let content = content.to_string();
        let commit_sha = commit_sha.map(str::to_string);

        tokio::task::spawn_blocking(move || {
            StateManager::new(session_path)
                .write_prince_verdict(&from, &to, &content, commit_sha.as_deref())
        })
        .await
        .map_err(|err| {
            StateError::Io(std::io::Error::other(format!(
                "Prince verdict write task failed: {err}"
            )))
        })?
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

    /// Atomically persist the session's typed work graph beside hierarchy.json.
    #[allow(dead_code)]
    pub fn write_work_graph(&self, graph: &WorkGraph) -> Result<(), StateError> {
        self.ensure_state_dir()?;

        let json = serde_json::to_string_pretty(graph)?;
        self.write_atomic_text(self.state_dir().join("work-graph.json"), &json)
    }

    /// Read the session's work graph. Legacy sessions have no graph file and
    /// return `None`, which is distinct from an explicitly empty graph.
    #[allow(dead_code)]
    pub fn read_work_graph(&self) -> Result<Option<WorkGraph>, StateError> {
        let path = self.state_dir().join("work-graph.json");
        if !path.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&json)?))
    }

    /// Refresh transition evidence and the standalone graph view from the current
    /// authoritative graph. This never writes `state/work-graph.json`; the scheduler
    /// remains its sole writer.
    pub fn emit_work_graph_snapshot(
        &self,
        lifecycle_stage: &str,
    ) -> Result<Option<PathBuf>, StateError> {
        let Some(graph) = self.read_work_graph()? else {
            return Ok(None);
        };
        let artifact = self.write_portable_work_graph_artifact(
            lifecycle_stage,
            &graph,
            None,
        )?;
        let snapshot = WorkGraphLifecycleSnapshot {
            lifecycle_stage: lifecycle_stage.to_string(),
            emitted_at: Utc::now(),
            node_count: graph.nodes.len(),
            edge_count: graph.edges.len(),
            artifact: "work-graph.html".to_string(),
        };
        self.ensure_state_dir()?;
        let json = serde_json::to_string_pretty(&snapshot)?;
        self.write_atomic_text(
            self.state_dir().join("work-graph-lifecycle.json"),
            &json,
        )?;
        Ok(Some(artifact))
    }

    pub fn read_work_graph_snapshot(
        &self,
    ) -> Result<Option<WorkGraphLifecycleSnapshot>, StateError> {
        let path = self.state_dir().join("work-graph-lifecycle.json");
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&json)?))
    }

    /// Write a browser-renderable, dependency-free graph artifact at the stable
    /// session-root path used by headless and post-crash operators.
    pub fn write_portable_work_graph_artifact(
        &self,
        lifecycle_stage: &str,
        graph: &WorkGraph,
        divergence: Option<&DivergenceSummary>,
    ) -> Result<PathBuf, StateError> {
        let nodes = graph
            .nodes
            .iter()
            .map(|node| {
                serde_json::json!({
                    "id": node.id,
                    "status": node.status,
                    "lane": node.binding,
                    "contract_summary": {
                        "input_count": node.contract.inputs.len(),
                        "output_count": node.contract.outputs.len(),
                        "acceptance_count": node.contract.acceptance.len(),
                    }
                })
            })
            .collect::<Vec<_>>();
        let edges = graph
            .edges
            .iter()
            .map(|edge| {
                serde_json::json!({
                    "source": edge.source,
                    "target": edge.target,
                    "kind": edge.kind,
                    "provenance": edge.provenance,
                })
            })
            .collect::<Vec<_>>();
        let session_id = self
            .session_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown-session");
        let payload = serde_json::json!({
            "session_id": session_id,
            "lifecycle_stage": lifecycle_stage,
            "generated_at": Utc::now(),
            "nodes": nodes,
            "edges": edges,
            "divergence": divergence,
        });
        let payload_json = serde_json::to_string(&payload)?
            .replace('&', "\\u0026")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e");
        let html = PORTABLE_WORK_GRAPH_HTML.replace("__GRAPH_DATA__", &payload_json);
        let path = self.session_path.join("work-graph.html");
        self.write_atomic_text(path.clone(), &html)?;
        Ok(path)
    }

    /// Persist evaluator-addressable template plus expansion state beside the
    /// authoritative graph. This sidecar is optional for legacy sessions.
    pub fn write_review_expansion_sidecar(
        &self,
        sidecar: &ReviewExpansionSidecar,
    ) -> Result<(), StateError> {
        self.ensure_state_dir()?;
        let json = serde_json::to_string_pretty(sidecar)?;
        self.write_atomic_text(self.state_dir().join("work-graph-reviews.json"), &json)
    }

    pub fn read_review_expansion_sidecar(
        &self,
    ) -> Result<Option<ReviewExpansionSidecar>, StateError> {
        let path = self.state_dir().join("work-graph-reviews.json");
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&json)?))
    }

    /// Persist the full phase-A/phase-B composition state without changing the
    /// legacy `work-graph.json` reader used by existing queue paths.
    pub fn write_graph_composition_state(
        &self,
        state: &GraphCompositionState,
    ) -> Result<(), StateError> {
        self.ensure_state_dir()?;
        let json = serde_json::to_string_pretty(state)?;
        self.write_atomic_text(self.state_dir().join("work-graph-composition.json"), &json)
    }

    pub fn read_graph_composition_state(
        &self,
    ) -> Result<Option<GraphCompositionState>, StateError> {
        let path = self.state_dir().join("work-graph-composition.json");
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&json)?))
    }

    /// Persist the visible orchestrator footprints, authority scopes, live
    /// principal ownership and surfaced write collisions for this session.
    pub fn write_ownership_session_state(
        &self,
        state: &OwnershipSessionState,
    ) -> Result<(), StateError> {
        self.ensure_state_dir()?;
        let json = serde_json::to_string_pretty(state)?;
        self.write_atomic_text(self.state_dir().join("orchestrator-ownership.json"), &json)
    }

    pub fn read_ownership_session_state(
        &self,
    ) -> Result<Option<OwnershipSessionState>, StateError> {
        let path = self.state_dir().join("orchestrator-ownership.json");
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&json)?))
    }

    /// Check and durably surface a collision before the caller mutates a path.
    /// Process write-capability is supplied explicitly; a task-status mirror is
    /// not accepted as a substitute for liveness (the d1e86179 failure mode).
    pub fn record_orchestrator_write_attempt(
        &self,
        graph: &WorkGraph,
        live_principals: &[LivePrincipal],
        attempt: OrchestratorWriteAttempt,
    ) -> Result<OrchestratorWriteOutcome, StateError> {
        let mut state = self.read_ownership_session_state()?.ok_or_else(|| {
            StateError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "orchestrator ownership state is unavailable",
            ))
        })?;
        state.live_principal_ownership = derive_path_ownership(graph, live_principals);
        let outcome = state.record_write_attempt(attempt);
        self.write_ownership_session_state(&state)?;
        Ok(outcome)
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

const PORTABLE_WORK_GRAPH_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Hive work graph</title>
  <style>
    :root { color-scheme: dark; font-family: ui-monospace, SFMono-Regular, Consolas, monospace; }
    body { margin: 0; padding: 24px; background: #090b10; color: #e7eaf0; }
    header { margin-bottom: 24px; }
    h1, h2, h3 { margin: 0 0 10px; }
    #meta { color: #9ca5b5; }
    #waves { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 16px; }
    .wave { padding: 14px; border: 1px solid #303747; border-radius: 10px; background: #121620; }
    .node { margin-top: 10px; padding: 10px; border-left: 4px solid #6aa9ff; background: #191f2c; }
    .node[data-status="blocked"], .node[data-status="failed"] { border-left-color: #ff6b6b; }
    .node small { display: block; margin-top: 6px; color: #aeb7c7; }
    table { width: 100%; margin-top: 12px; border-collapse: collapse; }
    th, td { padding: 8px; border-bottom: 1px solid #303747; text-align: left; }
    section { margin-top: 28px; }
    pre { overflow: auto; padding: 12px; background: #121620; border-radius: 8px; }
  </style>
</head>
<body>
  <header><h1>Work graph</h1><div id="meta"></div></header>
  <main>
    <div id="waves"></div>
    <section><h2>Edges</h2><table><thead><tr><th>Source</th><th>Target</th><th>Kind</th><th>Provenance</th></tr></thead><tbody id="edges"></tbody></table></section>
    <section><h2>Divergence</h2><pre id="divergence"></pre></section>
  </main>
  <script id="graph-data" type="application/json">__GRAPH_DATA__</script>
  <script>
    const data = JSON.parse(document.getElementById('graph-data').textContent);
    document.getElementById('meta').textContent = `${data.session_id} · ${data.lifecycle_stage} · ${data.generated_at}`;
    const nodeById = new Map(data.nodes.map(node => [node.id, node]));
    const indegree = new Map(data.nodes.map(node => [node.id, 0]));
    const dependents = new Map(data.nodes.map(node => [node.id, []]));
    for (const edge of data.edges.filter(edge => edge.kind === 'depends_on')) {
      if (!nodeById.has(edge.source) || !nodeById.has(edge.target)) continue;
      indegree.set(edge.target, indegree.get(edge.target) + 1);
      dependents.get(edge.source).push(edge.target);
    }
    let ready = [...indegree].filter(([, degree]) => degree === 0).map(([id]) => id).sort();
    const waves = [];
    while (ready.length) {
      const wave = ready;
      waves.push(wave);
      const next = new Set();
      for (const id of wave) {
        for (const target of dependents.get(id).sort()) {
          indegree.set(target, indegree.get(target) - 1);
          if (indegree.get(target) === 0) next.add(target);
        }
      }
      ready = [...next].sort();
    }
    const wavesRoot = document.getElementById('waves');
    waves.forEach((wave, index) => {
      const column = document.createElement('section');
      column.className = 'wave';
      const heading = document.createElement('h2');
      heading.textContent = `Wave ${index + 1}`;
      column.appendChild(heading);
      wave.forEach(id => {
        const node = nodeById.get(id);
        const card = document.createElement('article');
        card.className = 'node';
        card.dataset.status = node.status;
        const title = document.createElement('strong');
        title.textContent = `${node.id} · ${node.status}`;
        const detail = document.createElement('small');
        detail.textContent = `${node.lane.kind}:${node.lane.value} · ${node.contract_summary.input_count} in / ${node.contract_summary.output_count} out / ${node.contract_summary.acceptance_count} acceptance`;
        card.append(title, detail);
        column.appendChild(card);
      });
      wavesRoot.appendChild(column);
    });
    const edgeRoot = document.getElementById('edges');
    data.edges.forEach(edge => {
      const row = document.createElement('tr');
      [edge.source, edge.target, edge.kind, edge.provenance].forEach(value => {
        const cell = document.createElement('td');
        cell.textContent = value;
        row.appendChild(cell);
      });
      edgeRoot.appendChild(row);
    });
    document.getElementById('divergence').textContent = JSON.stringify(data.divergence || {}, null, 2);
  </script>
</body>
</html>
"##;

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
