//! Knowledge-context tests for issue #218, owned by WS-9.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use tempfile::TempDir;

use crate::orchestrator::work_graph::archetypes::GotchaAttachmentProvider;
use crate::orchestrator::work_graph::context::{
    context_node_is_stale, derive_project_context, ContextDerivationReport,
    NoTouchesResolver, ProjectKnowledgeGotchaProvider, TouchCoverageReport, TouchesResolver,
    ANTI_HUB_TASK_FRACTION, DERIVED_CONTEXT_TEMPLATE, MAX_CONTEXT_SCOPES_PER_GOTCHA,
    MAX_CONTEXT_SUMMARY_CHARS, MAX_DERIVED_CONTEXT_NODES,
};
use crate::orchestrator::work_graph::{
    BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus, TaskGraph,
    TaskId, WorkGraphOmissionReason, WorkNode,
};

#[derive(Clone)]
struct FakeTouches(BTreeMap<TaskId, BTreeSet<String>>);

impl TouchesResolver for FakeTouches {
    fn resolve_touches(
        &self,
        graph: &TaskGraph,
    ) -> Result<TouchCoverageReport, String> {
        let unresolved_task_ids = if self.0.is_empty() {
            Vec::new()
        } else {
            graph
                .nodes
                .iter()
                .filter(|node| node.kind == NodeKind::Task)
                .map(|node| node.id.clone())
                .filter(|task_id| !self.0.contains_key(task_id))
                .collect()
        };
        Ok(TouchCoverageReport {
            available: true,
            artifact_languages: BTreeSet::from(["fixture".to_string()]),
            touches: self.0.clone(),
            unresolved_task_ids,
        })
    }
}

#[test]
fn scoped_gotchas_do_not_trip_hub_lint_but_repo_wide_does() {
    let fixture = context_fixture();
    let mut graph = two_task_graph();
    let report = derive_project_context(&mut graph, &fixture.project, &fixture.touches);

    assert_eq!(ANTI_HUB_TASK_FRACTION, 0.75);
    assert_eq!(report.gotchas.len(), 3);
    assert_eq!(report.hub_lints.len(), 1);
    let repo_wide = report
        .gotchas
        .iter()
        .find(|gotcha| gotcha.scope == vec!["*"])
        .unwrap();
    let lint = &report.hub_lints[0];
    assert_eq!(lint.context_node_id, context_id(&repo_wide.id));
    assert_eq!(lint.linked_task_ids, vec!["task-x", "task-y"]);
    assert_eq!(lint.task_fraction, 1.0);
    for scoped in report.gotchas.iter().filter(|gotcha| gotcha.scope != vec!["*"]) {
        assert_ne!(lint.context_node_id, context_id(&scoped.id));
    }
    assert!(!graph.edges.iter().any(|edge| {
        edge.source == context_id(&repo_wide.id) && edge.kind == EdgeKind::Informs
    }));
}

#[test]
fn task_receives_exactly_gotchas_whose_scope_intersects_touches() {
    let fixture = context_fixture();
    let mut graph = two_task_graph();
    let report = derive_project_context(&mut graph, &fixture.project, &fixture.touches);

    assert_eq!(informed_scopes(&graph, &report, "task-x"), vec!["module/x"]);
    assert_eq!(informed_scopes(&graph, &report, "task-y"), vec!["module/y"]);
    assert_eq!(report.knowledge_edge_count, 2);
    assert!(graph.edges.iter().filter(|edge| edge.kind == EdgeKind::Informs).all(
        |edge| edge.provenance == EdgeProvenance::Knowledge
    ));
}

#[test]
fn missing_ai_docs_yields_no_knowledge_edges_and_stated_warning() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let mut graph = two_task_graph();

    let report = derive_project_context(&mut graph, &project, &NoTouchesResolver);

    assert!(!report.knowledge_available);
    assert_eq!(report.knowledge_edge_count, 0);
    assert!(!graph
        .edges
        .iter()
        .any(|edge| edge.provenance == EdgeProvenance::Knowledge));
    assert!(!graph.nodes.iter().any(|node| node.kind == NodeKind::Context));
    assert!(graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::ProjectKnowledgeUnavailable
            && omission
                .examples
                .iter()
                .any(|example| example.ends_with(".ai-docs"))
    }));
}

#[test]
fn missing_expected_source_is_reported_while_other_sources_are_derived() {
    let fixture = context_fixture();
    fs::remove_file(fixture.project.join(".ai-docs").join("bug-patterns.md")).unwrap();
    let mut graph = two_task_graph();

    let report = derive_project_context(&mut graph, &fixture.project, &fixture.touches);

    assert!(report.knowledge_available);
    assert_eq!(report.knowledge_edge_count, 2);
    assert!(graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::ProjectKnowledgeUnavailable
            && omission
                .examples
                .iter()
                .any(|example| example.contains("bug-patterns.md"))
    }));
}

#[test]
fn project_path_resolves_knowledge_when_worktree_has_no_ai_docs() {
    let fixture = context_fixture();
    let source_paths = [
        "project-dna.md",
        "bug-patterns.md",
        "learnings.jsonl",
        "curation-state.json",
    ];
    let before: Vec<_> = source_paths
        .iter()
        .map(|name| fs::read(fixture.project.join(".ai-docs").join(name)).unwrap())
        .collect();
    let worktree = fixture
        .project
        .join(".hive-manager")
        .join("worktrees")
        .join("session")
        .join("primary");
    fs::create_dir_all(&worktree).unwrap();
    assert!(!worktree.join(".ai-docs").exists());
    let mut graph = two_task_graph();

    let report = derive_project_context(&mut graph, &fixture.project, &fixture.touches);

    assert!(report.knowledge_available);
    assert_eq!(report.knowledge_edge_count, 2);
    assert!(!worktree.join(".ai-docs").exists());
    assert!(graph.nodes.iter().any(|node| node.kind == NodeKind::Context));
    let after: Vec<_> = source_paths
        .iter()
        .map(|name| fs::read(fixture.project.join(".ai-docs").join(name)).unwrap())
        .collect();
    assert_eq!(after, before, "derivation must never write project knowledge");
}

#[test]
fn absent_touches_resolver_reports_omission_instead_of_silent_clean() {
    let fixture = context_fixture();
    let mut graph = two_task_graph();

    let report = derive_project_context(&mut graph, &fixture.project, &NoTouchesResolver);

    assert!(report.knowledge_available);
    assert!(!report.touches_available);
    assert_eq!(report.knowledge_edge_count, 0);
    assert!(!graph.edges.iter().any(|edge| edge.kind == EdgeKind::Informs));
    assert!(graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::CodegraphUnavailable
            && omission.examples == vec!["touches-resolver"]
    }));
}

#[test]
fn successful_empty_touches_is_distinct_from_unavailable_resolution() {
    let fixture = context_fixture();
    let mut graph = two_task_graph();
    let resolved_empty = FakeTouches(BTreeMap::new());

    let report = derive_project_context(&mut graph, &fixture.project, &resolved_empty);

    assert!(report.touches_available);
    assert_eq!(report.knowledge_edge_count, 0);
    assert_eq!(report.hub_lints.len(), 1, "declared repo-wide context stays a hub");
    assert!(!graph.omissions.iter().any(|omission| {
        matches!(
            omission.reason,
            WorkGraphOmissionReason::CodegraphUnavailable
                | WorkGraphOmissionReason::ResolutionIncomplete
        )
    }));
}

#[test]
fn partial_touches_resolution_reports_the_missing_task() {
    let fixture = context_fixture();
    let mut graph = two_task_graph();
    let partial = FakeTouches(BTreeMap::from([(
        "task-x".to_string(),
        BTreeSet::from(["module/x".to_string()]),
    )]));

    let report = derive_project_context(&mut graph, &fixture.project, &partial);

    assert!(report.touches_available);
    assert!(graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::ResolutionIncomplete
            && omission.examples == vec!["task-y"]
    }));
    assert_eq!(informed_scopes(&graph, &report, "task-x"), vec!["module/x"]);
}

#[test]
fn bare_files_field_is_scoped_and_fenced_bug_template_is_ignored() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    let ai_docs = project.join(".ai-docs");
    fs::create_dir_all(&ai_docs).unwrap();
    fs::write(ai_docs.join("project-dna.md"), "# Project DNA\n").unwrap();
    fs::write(
        ai_docs.join("bug-patterns.md"),
        r#"# Bug Patterns

```markdown
## BUG-YYYY-NNN
- **Files**: *
- **Pattern**: Template, not a real bug.
```

## BUG-2026-001
- **Files**: module/x, module/y
- **Pattern**: Preserve the transaction boundary.
"#,
    )
    .unwrap();
    fs::write(ai_docs.join("learnings.jsonl"), "").unwrap();
    let mut graph = TaskGraph::new(vec![task("task-x")], Vec::new());
    let touches = FakeTouches(BTreeMap::from([(
        "task-x".to_string(),
        BTreeSet::from(["module/x".to_string()]),
    )]));

    let report = derive_project_context(&mut graph, &project, &touches);

    assert_eq!(report.gotchas.len(), 1);
    assert_eq!(report.gotchas[0].scope, vec!["module/x", "module/y"]);
    assert!(!report.gotchas[0].scope.contains(&"*".to_string()));
    assert_eq!(report.knowledge_edge_count, 1);
}

#[test]
fn scoped_learnings_win_the_bounded_node_budget() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    let ai_docs = project.join(".ai-docs");
    fs::create_dir_all(&ai_docs).unwrap();
    let mut dna = String::from("# Project DNA\n\n");
    for index in 0..=MAX_DERIVED_CONTEXT_NODES {
        dna.push_str(&format!(
            "## Unscoped entry {index}\nThis entry deliberately has no file evidence.\n\n"
        ));
    }
    fs::write(ai_docs.join("project-dna.md"), dna).unwrap();
    fs::write(ai_docs.join("bug-patterns.md"), "# Bug Patterns\n").unwrap();
    fs::write(
        ai_docs.join("learnings.jsonl"),
        r#"{"id":"scoped-learning","insight":"Keep module X atomic.","files_touched":["module/x"]}
"#,
    )
    .unwrap();
    fs::write(
        ai_docs.join("curation-state.json"),
        r#"{"last_curated_line":1}"#,
    )
    .unwrap();
    let mut graph = TaskGraph::new(vec![task("task-x")], Vec::new());
    let touches = FakeTouches(BTreeMap::from([(
        "task-x".to_string(),
        BTreeSet::from(["module/x".to_string()]),
    )]));

    let report = derive_project_context(&mut graph, &project, &touches);

    assert_eq!(report.gotchas.len(), MAX_DERIVED_CONTEXT_NODES);
    assert!(report
        .gotchas
        .iter()
        .any(|gotcha| gotcha.id == "scoped-learning"));
    assert_eq!(report.knowledge_edge_count, 1);
    assert!(graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::ResolutionIncomplete
            && omission.count == 2
    }));
}

#[test]
fn pending_learning_lines_are_not_exposed_as_curated_context() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    let ai_docs = project.join(".ai-docs");
    fs::create_dir_all(&ai_docs).unwrap();
    fs::write(ai_docs.join("project-dna.md"), "# Project DNA\n").unwrap();
    fs::write(ai_docs.join("bug-patterns.md"), "# Bug Patterns\n").unwrap();
    fs::write(
        ai_docs.join("learnings.jsonl"),
        r#"{"id":"curated","insight":"Curated X rule.","files_touched":["module/x"]}
{"id":"pending","insight":"Pending Y rule.","files_touched":["module/y"]}
"#,
    )
    .unwrap();
    fs::write(
        ai_docs.join("curation-state.json"),
        r#"{"last_curated_line":1}"#,
    )
    .unwrap();
    let mut graph = two_task_graph();
    let touches = FakeTouches(BTreeMap::from([
        (
            "task-x".to_string(),
            BTreeSet::from(["module/x".to_string()]),
        ),
        (
            "task-y".to_string(),
            BTreeSet::from(["module/y".to_string()]),
        ),
    ]));

    let report = derive_project_context(&mut graph, &project, &touches);

    assert_eq!(report.gotchas.len(), 1);
    assert_eq!(report.gotchas[0].id, "curated");
    assert_eq!(informed_scopes(&graph, &report, "task-x"), vec!["module/x"]);
    assert!(informed_scopes(&graph, &report, "task-y").is_empty());
}

#[test]
fn context_payload_is_bounded_and_detail_remains_addressable_by_source_ref() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    let ai_docs = project.join(".ai-docs");
    fs::create_dir_all(&ai_docs).unwrap();
    let long_summary = "x".repeat(MAX_CONTEXT_SUMMARY_CHARS * 4);
    fs::write(
        ai_docs.join("project-dna.md"),
        format!("# Project DNA\n\n### Long gotcha\n- **Scope**: `module/x`\n- {long_summary}\n"),
    )
    .unwrap();
    fs::write(ai_docs.join("bug-patterns.md"), "# Bug Patterns\n\n## Bugs\n").unwrap();
    fs::write(ai_docs.join("learnings.jsonl"), "").unwrap();
    let mut graph = TaskGraph::new(vec![task("task-x")], Vec::new());
    let touches = FakeTouches(BTreeMap::from([(
        "task-x".to_string(),
        BTreeSet::from(["module/x".to_string()]),
    )]));

    let report = derive_project_context(&mut graph, &project, &touches);

    let gotcha = &report.gotchas[0];
    assert_eq!(gotcha.summary.chars().count(), MAX_CONTEXT_SUMMARY_CHARS);
    assert!(gotcha.summary.ends_with('…'));
    assert!(gotcha.source_ref.starts_with(".ai-docs/project-dna.md#L"));
    assert!(!gotcha.summary.contains(&long_summary));
    let node = graph
        .nodes
        .iter()
        .find(|node| node.id == context_id(&gotcha.id))
        .unwrap();
    assert_eq!(node.title, gotcha.summary);
    assert!(node.contract.outputs.is_empty());
    assert_eq!(node.status, NodeStatus::Completed);
    assert_eq!(
        node.expansion
            .as_ref()
            .unwrap()
            .parameters
            .get("source_ref")
            .unwrap(),
        &gotcha.source_ref
    );
}

#[test]
fn context_scope_list_is_bounded_and_truncation_is_reported() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    let ai_docs = project.join(".ai-docs");
    fs::create_dir_all(&ai_docs).unwrap();
    fs::write(ai_docs.join("project-dna.md"), "# Project DNA\n").unwrap();
    fs::write(ai_docs.join("bug-patterns.md"), "# Bug Patterns\n").unwrap();
    let scopes: Vec<_> = (0..(MAX_CONTEXT_SCOPES_PER_GOTCHA + 4))
        .map(|index| format!("module/x/{index:02}"))
        .collect();
    fs::write(
        ai_docs.join("learnings.jsonl"),
        serde_json::json!({
            "id": "many-scopes",
            "insight": "Keep module X atomic.",
            "files_touched": scopes,
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        ai_docs.join("curation-state.json"),
        r#"{"last_curated_line":1}"#,
    )
    .unwrap();
    let mut graph = TaskGraph::new(vec![task("task-x")], Vec::new());
    let touches = FakeTouches(BTreeMap::from([(
        "task-x".to_string(),
        BTreeSet::from(["module/x".to_string()]),
    )]));

    let report = derive_project_context(&mut graph, &project, &touches);

    assert_eq!(report.gotchas[0].scope.len(), MAX_CONTEXT_SCOPES_PER_GOTCHA);
    assert_eq!(report.knowledge_edge_count, 1);
    assert!(graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::ResolutionIncomplete
            && omission.count == 4
            && omission.examples == vec![".ai-docs/learnings.jsonl#L1"]
    }));
}

#[test]
fn global_cross_ref_is_bounded_and_inherits_only_explicit_parent_scope() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    let ai_docs = project.join(".ai-docs");
    fs::create_dir_all(&ai_docs).unwrap();
    fs::write(
        ai_docs.join("project-dna.md"),
        "# Project DNA\n\n### Scoped pattern\n- **Scope**: `module/x`\n- Keep X safe.\n- -> global: [Detail](../../.ai-docs/wiki/patterns/safe-x.md) § gate\n",
    )
    .unwrap();
    fs::write(ai_docs.join("bug-patterns.md"), "# Bug Patterns\n\n## Bugs\n").unwrap();
    fs::write(ai_docs.join("learnings.jsonl"), "").unwrap();
    let mut graph = TaskGraph::new(vec![task("task-x")], Vec::new());
    let touches = FakeTouches(BTreeMap::from([(
        "task-x".to_string(),
        BTreeSet::from(["module/x".to_string()]),
    )]));

    let report = derive_project_context(&mut graph, &project, &touches);
    let global = report
        .gotchas
        .iter()
        .find(|gotcha| gotcha.source_ref.starts_with("global:"))
        .unwrap();

    assert_eq!(global.source_ref, "global:patterns/safe-x.md");
    assert_eq!(global.scope, vec!["module/x"]);
    assert!(global.summary.chars().count() <= MAX_CONTEXT_SUMMARY_CHARS);
    assert!(graph.edges.iter().any(|edge| {
        edge.source == context_id(&global.id)
            && edge.target == "task-x"
            && edge.kind == EdgeKind::Informs
    }));
}

#[test]
fn content_hash_makes_existing_context_node_detectably_stale() {
    let fixture = context_fixture();
    let mut graph = two_task_graph();
    let report = derive_project_context(&mut graph, &fixture.project, &fixture.touches);
    let node = graph
        .nodes
        .iter()
        .find(|node| {
            node.expansion
                .as_ref()
                .is_some_and(|expansion| expansion.template == DERIVED_CONTEXT_TEMPLATE)
        })
        .unwrap();
    assert!(!context_node_is_stale(node, &report.source_fingerprints));

    let mut changed = report.source_fingerprints.clone();
    let dna = changed
        .iter_mut()
        .find(|fingerprint| fingerprint.source_ref == ".ai-docs/project-dna.md")
        .unwrap();
    dna.content_hash = "changed".to_string();
    assert!(context_node_is_stale(node, &changed));
}

#[test]
fn project_knowledge_provider_implements_frozen_archetype_seam() {
    let fixture = context_fixture();
    let graph = two_task_graph();
    let provider = ProjectKnowledgeGotchaProvider {
        graph: &graph,
        resolver: fixture.touches,
    };

    let attachments = provider.gotchas(&fixture.project).unwrap().unwrap();

    assert_eq!(attachments.len(), 2);
    assert_eq!(
        attachments
            .iter()
            .map(|attachment| attachment.lane_id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["task-x", "task-y"])
    );
}

struct ContextFixture {
    _temp: TempDir,
    project: std::path::PathBuf,
    touches: FakeTouches,
}

fn context_fixture() -> ContextFixture {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    let ai_docs = project.join(".ai-docs");
    fs::create_dir_all(&ai_docs).unwrap();
    fs::write(
        ai_docs.join("project-dna.md"),
        r#"# Project DNA

## Patterns That Work

### X transaction boundary
- **Scope**: `module/x`
- Keep X changes inside one transaction.

### Y authorization boundary
- **Scope**: `module/y`
- Reapply Y authorization at every entity path.

### Universal ceremony
- **Scope**: `*`
- Say hello before every task.
"#,
    )
    .unwrap();
    fs::write(ai_docs.join("bug-patterns.md"), "# Bug Patterns\n\n## Bugs\n").unwrap();
    fs::write(ai_docs.join("learnings.jsonl"), "").unwrap();
    fs::write(
        ai_docs.join("curation-state.json"),
        r#"{"last_curated_line":0}"#,
    )
    .unwrap();
    ContextFixture {
        _temp: temp,
        project,
        touches: FakeTouches(BTreeMap::from([
            (
                "task-x".to_string(),
                BTreeSet::from(["module/x".to_string()]),
            ),
            (
                "task-y".to_string(),
                BTreeSet::from(["module/y".to_string()]),
            ),
        ])),
    }
}

fn two_task_graph() -> TaskGraph {
    TaskGraph::new(vec![task("task-x"), task("task-y")], Vec::new())
}

fn task(id: &str) -> WorkNode {
    WorkNode::new(
        id,
        NodeKind::Task,
        format!("Task {id}"),
        NodeContract::default(),
        BindingRef::Role("backend".to_string()),
        NodeStatus::Pending,
    )
}

fn context_id(gotcha_id: &str) -> String {
    format!("context::knowledge::{gotcha_id}")
}

fn informed_scopes(
    graph: &TaskGraph,
    report: &ContextDerivationReport,
    task_id: &str,
) -> Vec<String> {
    let by_id: BTreeMap<_, _> = report
        .gotchas
        .iter()
        .map(|gotcha| (context_id(&gotcha.id), gotcha.scope.join(",")))
        .collect();
    let mut scopes: Vec<_> = graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Informs && edge.target == task_id)
        .filter_map(|edge| by_id.get(&edge.source).cloned())
        .collect();
    scopes.sort();
    scopes
}
