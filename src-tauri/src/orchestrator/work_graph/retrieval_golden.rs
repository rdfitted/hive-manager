use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::actions::coordination::parse_plan_markdown_with_diagnostics;

use super::codegraph::ArtifactCodegraph;
use super::context::{
    build_knowledge_touch_coverage, prepare_knowledge_candidate_selection,
    ContextDerivationReport, KnowledgeAttachmentConfig,
};
use super::plan_parse::task_graph_from_plan;
use super::runtime::{
    compose_initial_work_graph, derive_plan_ready_knowledge_attachments,
};
use super::{EdgeKind, EdgeProvenance, TaskGraph, TaskId, WorkGraphOmission};

const FIXTURES: [&str; 13] = [
    "declared-scope",
    "star-hub",
    "codegraph-unavailable",
    "file-list-fallback",
    "harvested-line-range",
    "inferred-ambiguous",
    "inferred-basename",
    "inferred-exact",
    "inferred-stale",
    "inferred-suffix",
    "mixed-intent-failures",
    "partial-task",
    "star-hub-unavailable",
];
const KNOWLEDGE_FILES: [&str; 4] = [
    "project-dna.md",
    "bug-patterns.md",
    "learnings.jsonl",
    "curation-state.json",
];
const GOLDEN_SCHEMA: &str = "hive-retrieval-golden/v1";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RetrievalGolden {
    schema: String,
    fixtures: BTreeMap<String, FixtureGolden>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FixtureGolden {
    entry: String,
    parsed_note_count: usize,
    declared_touches: BTreeMap<TaskId, Vec<String>>,
    knowledge_attachment_touches: BTreeMap<TaskId, Vec<String>>,
    knowledge_edges: Vec<GoldenKnowledgeEdge>,
    context_nodes: Vec<GoldenContextNode>,
    omissions: Vec<GoldenOmission>,
    hub_lints: Vec<GoldenHubLint>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GoldenKnowledgeEdge {
    task_id: TaskId,
    context_node_id: TaskId,
    rationale: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GoldenContextNode {
    id: TaskId,
    title: String,
    summary: String,
    scope: Vec<String>,
    parameters: BTreeMap<String, String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GoldenOmission {
    reason: String,
    count: usize,
    detail: String,
    examples: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GoldenHubLint {
    context_node_id: TaskId,
    linked_task_ids: Vec<TaskId>,
    reason: String,
}

struct MaterializedFixture {
    _temp: TempDir,
    root: PathBuf,
    plan: String,
    resolver: ArtifactCodegraph,
    file_inventory: BTreeSet<String>,
    entry: String,
}

struct PipelineOutput {
    graph: TaskGraph,
    context: ContextDerivationReport,
    declared_touches: BTreeMap<TaskId, BTreeSet<String>>,
    knowledge_attachment_touches: BTreeMap<TaskId, BTreeSet<String>>,
}

fn run_pipeline(
    plan_markdown: &str,
    root: &Path,
    resolver: &ArtifactCodegraph,
    file_inventory: &BTreeSet<String>,
    entry: &str,
) -> PipelineOutput {
    let (plan, diagnostics) = parse_plan_markdown_with_diagnostics(plan_markdown);
    assert!(
        diagnostics.is_empty(),
        "synthetic plan must parse without diagnostics: {diagnostics:?}"
    );
    let mut graph = task_graph_from_plan(&plan);
    if entry == "compose" {
        let state = compose_initial_work_graph(
            graph,
            root,
            None,
            None,
            &BTreeMap::new(),
            resolver,
        )
        .expect("synthetic compose-path retrieval pipeline should compose");
        let declared_coverage = state.codegraph.coverage();
        let config = KnowledgeAttachmentConfig::production();
        let selection =
            prepare_knowledge_candidate_selection(resolver, root, None, config);
        let knowledge = build_knowledge_touch_coverage(
            &state.graph,
            &declared_coverage,
            &selection,
            config,
        );
        return PipelineOutput {
            graph: state.graph,
            context: state.context,
            declared_touches: declared_coverage.touches,
            knowledge_attachment_touches: knowledge.knowledge_attachment_touches,
        };
    }
    assert_eq!(entry, "plan-ready", "fixture entry must be compose or plan-ready");
    let result = derive_plan_ready_knowledge_attachments(
        &mut graph,
        root,
        None,
        root,
        resolver,
        Some(file_inventory),
    )
    .expect("synthetic plan contains task nodes");
    PipelineOutput {
        graph,
        context: result.context,
        declared_touches: result.transient_declared_touches,
        knowledge_attachment_touches: result.knowledge_attachment_touches,
    }
}

#[test]
fn retrieval_golden_matches_current_rust_rule() {
    let actual = build_golden();
    let golden_path = judgment_root().join("golden/retrieval-golden.json");

    if std::env::var("UPDATE_RETRIEVAL_GOLDEN").as_deref() == Ok("1") {
        assert!(
            std::env::var_os("CI").is_none(),
            "retrieval golden regeneration must never run in CI"
        );
        let json =
            serde_json::to_string_pretty(&actual).expect("retrieval golden should serialize");
        fs::write(&golden_path, format!("{json}\n"))
            .expect("retrieval golden should be writable in update mode");
        return;
    }

    let expected_text =
        fs::read_to_string(&golden_path).expect("checked-in retrieval golden should be readable");
    let expected: RetrievalGolden = serde_json::from_str(&expected_text)
        .expect("checked-in retrieval golden should match its schema");
    assert_eq!(
        actual, expected,
        "retrieval behavior drifted; regenerate only after reviewing the Rust rule change"
    );
}

fn build_golden() -> RetrievalGolden {
    let mut fixtures = BTreeMap::new();
    for name in FIXTURES {
        let materialized = materialize_fixture(name);
        let state = run_pipeline(
            &materialized.plan,
            &materialized.root,
            &materialized.resolver,
            &materialized.file_inventory,
            &materialized.entry,
        );
        let fixture = fixture_golden(&state, &materialized.root, &materialized.entry);
        assert!(
            fixture.parsed_note_count > 0,
            "fixture {name} must parse at least one knowledge note"
        );
        fixtures.insert(name.to_string(), fixture);
    }

    let declared = &fixtures["declared-scope"];
    assert!(
        !declared.knowledge_edges.is_empty(),
        "declared-scope must produce a knowledge edge"
    );
    assert!(declared.declared_touches.contains_key("T1"));
    assert!(
        !declared.declared_touches.contains_key("T2"),
        "one unresolved intent must void all touches for T2"
    );
    assert!(declared
        .omissions
        .iter()
        .any(|omission| omission.examples.iter().any(|example| example == "T2")));

    let star_hub = &fixtures["star-hub"];
    assert!(
        star_hub.knowledge_edges.is_empty() && !star_hub.hub_lints.is_empty(),
        "star scope must be withheld as a hub"
    );

    let unavailable = &fixtures["codegraph-unavailable"];
    assert!(unavailable.declared_touches.is_empty());
    assert!(unavailable
        .omissions
        .iter()
        .any(|omission| omission.reason == "codegraph_unavailable"));

    RetrievalGolden {
        schema: GOLDEN_SCHEMA.to_string(),
        fixtures,
    }
}

fn fixture_golden(state: &PipelineOutput, root: &Path, entry: &str) -> FixtureGolden {
    let declared_touches = touch_map(&state.declared_touches);
    let knowledge_edges = state
        .graph
        .edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::Informs && edge.provenance == EdgeProvenance::Knowledge
        })
        .map(|edge| GoldenKnowledgeEdge {
            task_id: edge.target.clone(),
            context_node_id: edge.source.clone(),
            rationale: edge.rationale.clone().unwrap_or_default(),
        })
        .collect();
    let omissions = state
        .graph
        .omissions
        .iter()
        .map(|omission| golden_omission(omission, root))
        .collect();
    let context_nodes = state
        .context
        .gotchas
        .iter()
        .map(|gotcha| {
            let id = format!("context::knowledge::{}", gotcha.id);
            let node = state
                .graph
                .nodes
                .iter()
                .find(|node| node.id == id)
                .expect("reported gotcha must have a context node");
            GoldenContextNode {
                id,
                title: node.title.clone(),
                summary: gotcha.summary.clone(),
                scope: gotcha.scope.clone(),
                parameters: node
                    .expansion
                    .as_ref()
                    .expect("derived context node must carry expansion parameters")
                    .parameters
                    .clone(),
            }
        })
        .collect();
    let hub_lints = state
        .context
        .hub_lints
        .iter()
        .map(|lint| GoldenHubLint {
            context_node_id: lint.context_node_id.clone(),
            linked_task_ids: lint.linked_task_ids.clone(),
            reason: lint.detail.clone(),
        })
        .collect();

    FixtureGolden {
        entry: entry.to_string(),
        parsed_note_count: state.context.gotchas.len(),
        knowledge_attachment_touches: touch_map(&state.knowledge_attachment_touches),
        declared_touches,
        knowledge_edges,
        context_nodes,
        omissions,
        hub_lints,
    }
}

fn touch_map(touches: &BTreeMap<TaskId, BTreeSet<String>>) -> BTreeMap<TaskId, Vec<String>> {
    touches
        .iter()
        .map(|(task_id, paths)| (task_id.clone(), paths.iter().cloned().collect()))
        .collect()
}

fn golden_omission(omission: &WorkGraphOmission, root: &Path) -> GoldenOmission {
    GoldenOmission {
        reason: serde_json::to_value(omission.reason)
            .expect("omission reason should serialize")
            .as_str()
            .expect("omission reason should serialize as a string")
            .to_string(),
        count: omission.count,
        detail: normalize_root(&omission.detail, root),
        examples: omission
            .examples
            .iter()
            .map(|example| normalize_root(example, root))
            .collect(),
    }
}

fn materialize_fixture(name: &str) -> MaterializedFixture {
    let source = judgment_root().join("fixtures").join(name);
    assert!(
        source.is_dir(),
        "fixture directory is missing: {}",
        source.display()
    );

    let temp = TempDir::new().expect("fixture TempDir should be created");
    let project = temp.path().join("project");
    let ai_docs = project.join(".ai-docs");
    fs::create_dir_all(&ai_docs).expect("materialized .ai-docs should be created");

    let plan = normalized_text(&source.join("plan.md"));
    fs::write(project.join("plan.md"), &plan).expect("materialized plan should be written");
    let files = normalized_text(&source.join("files.txt"));
    fs::write(project.join("files.txt"), &files)
        .expect("materialized file list should be written");
    let file_inventory = files
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| path.replace('\\', "/"))
        .collect();
    let entry = normalized_text(&source.join("entry.txt")).trim().to_string();
    assert!(
        matches!(entry.as_str(), "compose" | "plan-ready"),
        "fixture entry must be compose or plan-ready"
    );
    for filename in KNOWLEDGE_FILES {
        let content = normalized_text(&source.join("ai-docs").join(filename));
        fs::write(ai_docs.join(filename), content)
            .expect("materialized knowledge file should be written");
    }

    let root = fs::canonicalize(&project).expect("materialized root should canonicalize");
    let artifact_source = source.join("codegraph.json");
    let resolver = if artifact_source.is_file() {
        let artifact_text = normalized_text(&artifact_source);
        let mut artifact: serde_json::Value =
            serde_json::from_str(&artifact_text).expect("fixture codegraph should be valid JSON");
        assert_eq!(
            artifact.get("root").and_then(serde_json::Value::as_str),
            Some("<ROOT>"),
            "fixture codegraph root must use the placeholder"
        );
        artifact["root"] = serde_json::Value::String(root.display().to_string());
        let artifact_text = serde_json::to_string_pretty(&artifact)
            .expect("materialized codegraph should serialize");
        fs::write(project.join("codegraph.json"), &artifact_text)
            .expect("materialized codegraph should be written");
        ArtifactCodegraph::from_json(&root, &artifact_text)
            .expect("materialized codegraph should load")
    } else {
        assert!(
            source.join("none").is_file(),
            "fixture must contain codegraph.json or none"
        );
        ArtifactCodegraph::load(&root, &project.join("codegraph.json"))
            .expect("missing materialized codegraph should be unavailable")
    };

    MaterializedFixture {
        _temp: temp,
        root,
        plan,
        resolver,
        file_inventory,
        entry,
    }
}

fn normalized_text(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| {
            panic!(
                "fixture text {} should be readable: {error}",
                path.display()
            )
        })
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn normalize_root(value: &str, root: &Path) -> String {
    let native = root.display().to_string();
    value
        .replace(&native, "<ROOT>")
        .replace(&native.replace('\\', "/"), "<ROOT>")
}

fn judgment_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri should have a repository parent")
        .join("tools/judgment")
}
