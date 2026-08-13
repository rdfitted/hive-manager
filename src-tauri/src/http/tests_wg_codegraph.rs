//! `/codegraph` artifact integration tests for issue #215.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use tempfile::TempDir;

use crate::domain::WorkspaceStrategy;
use crate::orchestrator::work_graph::archetypes::RepoShapeFactsProvider;
use crate::orchestrator::work_graph::codegraph::{
    conflicting_ready_tasks, derive_codegraph_touches, ArtifactCodegraph,
    ConflictDetectionState, ParallelConflictAction, CODEGRAPH_MODULE_TEMPLATE,
};
use crate::orchestrator::work_graph::{
    BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus,
    TaskGraph, WorkEdge, WorkGraphOmissionReason, WorkNode,
};

#[test]
fn available_artifact_adds_codegraph_touches_edges() {
    let fixture = CodegraphFixture::new();
    let codegraph = fixture.codegraph();
    let mut graph = TaskGraph::new(
        vec![task("T1", NodeStatus::Ready, &["touch:src/lib/domain.ts"])],
        Vec::new(),
    );

    let report = derive_codegraph_touches(&mut graph, &codegraph);

    assert!(report.available);
    assert_eq!(report.touches["T1"], BTreeSet::from(["src/lib/domain.ts".to_string()]));
    assert_eq!(report.module_node_count, 1);
    assert_eq!(report.touch_edge_count, 1);
    let module = graph
        .nodes
        .iter()
        .find(|node| {
            node.expansion
                .as_ref()
                .is_some_and(|expansion| expansion.template == CODEGRAPH_MODULE_TEMPLATE)
        })
        .unwrap();
    assert_eq!(module.kind, NodeKind::Context);
    assert_eq!(module.status, NodeStatus::Completed);
    assert_eq!(
        module.expansion.as_ref().unwrap().parameters["module"],
        "src/lib/domain.ts"
    );
    let edge = graph
        .edges
        .iter()
        .find(|edge| edge.kind == EdgeKind::Touches)
        .unwrap();
    assert_eq!(edge.source, "T1");
    assert_eq!(edge.target, module.id);
    assert_eq!(edge.provenance, EdgeProvenance::Codegraph);
    assert!(edge
        .rationale
        .as_deref()
        .unwrap()
        .contains("explicit task intent"));

    let second = derive_codegraph_touches(&mut graph, &codegraph);
    assert_eq!(second.touch_edge_count, 1);
    assert_eq!(
        graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Touches)
            .count(),
        1,
        "re-derivation must be idempotent"
    );
}

#[test]
fn absent_artifact_disables_touches_and_reports_omission() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let codegraph = ArtifactCodegraph::load(&project, &project.join("missing.json")).unwrap();
    let mut graph = TaskGraph::new(
        vec![task("T1", NodeStatus::Ready, &["touch:src/lib/domain.ts"])],
        Vec::new(),
    );

    let report = derive_codegraph_touches(&mut graph, &codegraph);
    let detection = conflicting_ready_tasks(&graph, None, WorkspaceStrategy::SharedCell);

    assert!(!report.available);
    assert_eq!(report.touch_edge_count, 0);
    assert_eq!(detection.state, ConflictDetectionState::Disabled);
    assert!(detection.decisions.is_empty());
    assert!(!graph.edges.iter().any(|edge| edge.kind == EdgeKind::Touches));
    assert!(graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::CodegraphUnavailable
            && omission.examples == vec!["touches-resolver"]
    }));
}

#[test]
fn ready_overlap_returns_logged_serialization_reason() {
    let fixture = CodegraphFixture::new();
    let codegraph = fixture.codegraph();
    let mut graph = TaskGraph::new(
        vec![
            task("T1", NodeStatus::Ready, &["touch:src/lib/domain.ts"]),
            task("T2", NodeStatus::Ready, &["file:src/lib/domain.ts"]),
            task("T3", NodeStatus::Ready, &["module:src/lib/domain.ts"]),
            WorkNode::new(
                "gate",
                NodeKind::Checkpoint,
                "Gate",
                NodeContract::default(),
                BindingRef::Zone("operator".to_string()),
                NodeStatus::Pending,
            ),
        ],
        vec![WorkEdge::new(
            "gate",
            "T3",
            EdgeKind::DependsOn,
            EdgeProvenance::Planner,
        )],
    );
    let report = derive_codegraph_touches(&mut graph, &codegraph);

    let conflicts = conflicting_ready_tasks(
        &graph,
        Some(&report.touches),
        WorkspaceStrategy::SharedCell,
    );

    assert_eq!(conflicts.state, ConflictDetectionState::Complete);
    assert_eq!(conflicts.decisions.len(), 1);
    let decision = &conflicts.decisions[0];
    assert_eq!(decision.first_task_id, "T1");
    assert_eq!(decision.second_task_id, "T2");
    assert_eq!(decision.overlapping_modules, vec!["src/lib/domain.ts"]);
    assert_eq!(decision.action, ParallelConflictAction::Serialize);
    assert_eq!(
        decision.reason,
        "ready tasks T1 and T2 overlap codegraph modules [src/lib/domain.ts]; serialize claims because workspace strategy is shared_cell"
    );

    let isolated = conflicting_ready_tasks(
        &graph,
        Some(&report.touches),
        WorkspaceStrategy::IsolatedCell,
    );
    assert_eq!(
        isolated.decisions[0].action,
        ParallelConflictAction::WorktreeIsolate
    );
    assert!(isolated.decisions[0]
        .reason
        .ends_with("isolate claims because workspace strategy is isolated_cell"));
}

#[test]
fn worktree_and_generated_modules_are_excluded_from_artifact() {
    let fixture = CodegraphFixture::new();
    let codegraph = fixture.codegraph();

    assert_eq!(
        codegraph.indexed_modules(),
        &BTreeSet::from([
            "src/lib/domain.ts".to_string(),
            "src/routes/+page.ts".to_string(),
        ])
    );
    assert!(!codegraph
        .indexed_modules()
        .iter()
        .any(|module| module.contains("phantom") || module.contains("worktrees")));
    assert!(!codegraph
        .indexed_modules()
        .iter()
        .any(|module| module.starts_with(".svelte-kit")));
}

#[test]
fn partial_resolution_reports_uncovered_rust_language() {
    let fixture = CodegraphFixture::new();
    let codegraph = fixture.codegraph();
    let mut graph = TaskGraph::new(
        vec![
            task("T1", NodeStatus::Ready, &["touch:src/lib/domain.ts"]),
            task(
                "T2",
                NodeStatus::Ready,
                &["touch:src-tauri/src/session/controller.rs"],
            ),
        ],
        Vec::new(),
    );

    let report = derive_codegraph_touches(&mut graph, &codegraph);
    let conflicts = conflicting_ready_tasks(
        &graph,
        Some(&report.touches),
        WorkspaceStrategy::SharedCell,
    );

    assert_eq!(report.unresolved_task_ids, vec!["T2"]);
    assert_eq!(conflicts.state, ConflictDetectionState::Partial);
    assert_eq!(conflicts.unresolved_ready_task_ids, vec!["T2"]);
    let omission = graph
        .omissions
        .iter()
        .find(|omission| {
            omission.reason == WorkGraphOmissionReason::ResolutionIncomplete
                && omission.examples == vec!["T2"]
        })
        .unwrap();
    assert_eq!(
        omission.detail,
        "codegraph artifact was available but did not cover or resolve declared language(s): rust"
    );
    assert!(conflicts.decisions.is_empty());
}

#[test]
fn available_artifact_without_explicit_intent_is_not_false_clean() {
    let fixture = CodegraphFixture::new();
    let codegraph = fixture.codegraph();
    let mut graph = TaskGraph::new(
        vec![task("T1", NodeStatus::Ready, &["implement the frontend"])],
        Vec::new(),
    );

    let report = derive_codegraph_touches(&mut graph, &codegraph);

    assert!(report.available);
    assert_eq!(report.unresolved_task_ids, vec!["T1"]);
    assert_eq!(report.touch_edge_count, 0);
    let omission = graph
        .omissions
        .iter()
        .find(|omission| omission.examples == vec!["T1"])
        .unwrap();
    assert_eq!(omission.reason, WorkGraphOmissionReason::ResolutionIncomplete);
    assert_eq!(
        omission.detail,
        "codegraph artifact was available, but explicit task touch intent was not declared"
    );
}

#[test]
fn explicit_none_is_a_successful_empty_resolution() {
    let fixture = CodegraphFixture::new();
    let codegraph = fixture.codegraph();
    let mut graph = TaskGraph::new(
        vec![task("T1", NodeStatus::Ready, &["touch:none"])],
        Vec::new(),
    );

    let report = derive_codegraph_touches(&mut graph, &codegraph);
    let detection = conflicting_ready_tasks(
        &graph,
        Some(&report.touches),
        WorkspaceStrategy::SharedCell,
    );

    assert_eq!(report.touches, BTreeMap::from([("T1".to_string(), BTreeSet::new())]));
    assert!(report.unresolved_task_ids.is_empty());
    assert!(graph.omissions.is_empty());
    assert_eq!(detection.state, ConflictDetectionState::Complete);
    assert!(detection.decisions.is_empty());
}

#[test]
fn artifact_repo_shape_facts_drive_frozen_provider() {
    let fixture = CodegraphFixture::new();
    let codegraph = fixture.codegraph();

    let facts = codegraph.facts(&fixture.project).unwrap().unwrap();

    assert!(facts.facts.contains("codegraph:typescript"));
    assert!(facts.facts.contains("language:typescript/javascript"));
    assert!(facts.facts.contains("frontend"));
    assert!(!facts.facts.contains("backend"));
}

struct CodegraphFixture {
    _temp: TempDir,
    project: std::path::PathBuf,
    artifact_json: String,
}

impl CodegraphFixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let root = fs::canonicalize(&project).unwrap();
        let artifact_json = serde_json::json!({
            "root": root,
            "language": "typescript",
            "counts": {"modules": 8, "edges": 0},
            "edges": [],
            "unresolved": {},
            "parse_errors": {},
            "aliases": {},
            "opaque_prefixes": [],
            "opaque_unbounded": false,
            "opaque_sites": [],
            "nodes": {
                "src/lib/domain.ts": node_json("src/lib/domain.ts"),
                "src/routes/+page.ts": node_json("src/routes/+page.ts"),
                ".svelte-kit/generated/client/app.js": node_json(".svelte-kit/generated/client/app.js"),
                ".hive-manager/worktrees/session/primary/src/lib/domain.ts": node_json(".hive-manager/worktrees/session/primary/src/lib/domain.ts"),
                ".hive-manager/worktrees/session/primary/src/lib/phantom.ts": node_json(".hive-manager/worktrees/session/primary/src/lib/phantom.ts"),
                ".hive-fusion/session/variant/src/lib/fusion-phantom.ts": node_json(".hive-fusion/session/variant/src/lib/fusion-phantom.ts"),
                ".hive-debate/session/debater/src/lib/debate-phantom.ts": node_json(".hive-debate/session/debater/src/lib/debate-phantom.ts"),
                "node_modules/pkg/index.js": node_json("node_modules/pkg/index.js")
            }
        })
        .to_string();
        Self {
            _temp: temp,
            project,
            artifact_json,
        }
    }

    fn codegraph(&self) -> ArtifactCodegraph {
        ArtifactCodegraph::from_json(&self.project, &self.artifact_json).unwrap()
    }
}

fn node_json(path: &str) -> serde_json::Value {
    serde_json::json!({
        "path": path,
        "loc": 1,
        "entrypoint": [],
        "runnable": false,
        "reachable": true,
        "importers": [],
        "imports": []
    })
}

fn task(id: &str, status: NodeStatus, inputs: &[&str]) -> WorkNode {
    WorkNode::new(
        id,
        NodeKind::Task,
        format!("Task {id}"),
        NodeContract {
            inputs: inputs.iter().map(|input| (*input).to_string()).collect(),
            outputs: Vec::new(),
            acceptance: Vec::new(),
        },
        BindingRef::Role("backend".to_string()),
        status,
    )
}
