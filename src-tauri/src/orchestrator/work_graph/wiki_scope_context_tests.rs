//! Wiki label scope at worker-context derivation (schema.md §11, strict order
//! per Ryan 2026-11-04). Fixtures live in temp dirs only, never the real wiki.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use tempfile::TempDir;

use super::{
    derive_project_context_from_knowledge, KnowledgeAttachmentConfig, KnowledgeTouchCoverage,
    TouchCoverageReport, EXCLUDED_CONTEXT_PARAMETER,
};
use crate::orchestrator::org_graph::composition::{
    compose_context, knowledge_refs_from_work_graph, render_composed_context,
    spawn_context_from_work_graph_task, ContextDropReason, SpawnContext,
};
use crate::orchestrator::org_graph::KnowledgeSource;
use crate::orchestrator::work_graph::{
    BindingRef, NodeContract, NodeKind, NodeStatus, TaskGraph, WorkNode,
};

const PARITY: &str = include_str!("testdata/wiki_scope_parity.json");
const CLIENT_SECRET: &str = "TOG pricing sheet lives in the shared drive";

struct Fixture {
    _dir: TempDir,
    project: PathBuf,
    wiki: PathBuf,
}

/// `<tmp>/Code Projects/<repo>/.ai-docs/project-dna.md` with one scoped
/// section linking `links`, plus a wiki carrying the parity label map and
/// the named pages.
fn fixture(repo: &str, links: &[&str], with_map: bool) -> Fixture {
    let dir = TempDir::new().expect("tempdir");
    let project = dir.path().join("Code Projects").join(repo);
    fs::create_dir_all(project.join(".ai-docs")).expect("ai-docs");
    let mut dna = String::from("## Wiki links\n**Scope**: `src/main.rs`\nSee the linked pages.\n");
    for link in links {
        dna.push_str(&format!("-> global: {link}\n"));
    }
    fs::write(project.join(".ai-docs/project-dna.md"), dna).expect("dna");

    let wiki = dir.path().join("wiki");
    let page = |path: &str, frontmatter: &str| {
        let full = wiki.join(path);
        fs::create_dir_all(full.parent().unwrap()).expect("wiki dir");
        fs::write(full, format!("---\n{frontmatter}---\n# Body\n")).expect("wiki page");
    };
    page(
        "clients/tog.md",
        &format!("title: TOG\nscope: client:tog\ndescription: {CLIENT_SECRET}\n"),
    );
    page(
        "clients/egi.md",
        "title: EGI\nscope: client:egi\ndescription: EGI rollout notes\n",
    );
    page(
        "clients/axiom-coherence.md",
        "title: Axiom\nscope: client:axiom-coherence\ndescription: Office notes\n",
    );
    page(
        "clients/new-client.md",
        "title: New client\nscope: client:new-client\ndescription: Fresh client page\n",
    );
    page(
        "patterns/guide.md",
        "title: Reliable planning\nscope: universal\ndescription: Keep evidence attached.\n",
    );
    page(
        "practices/added-after-map.md",
        "title: Late page\nscope: universal\ndescription: Added after the map build.\n",
    );
    page("practices/no-scope.md", "title: Unlabelled\ndescription: No scope line.\n");
    if with_map {
        let parity: Value = serde_json::from_str(PARITY).unwrap();
        fs::create_dir_all(wiki.join("derived")).unwrap();
        fs::write(
            wiki.join("derived/labels.json"),
            serde_json::to_string_pretty(&parity["label_map"]).unwrap(),
        )
        .unwrap();
    }
    Fixture {
        _dir: dir,
        project,
        wiki,
    }
}

fn derive(fixture: &Fixture) -> TaskGraph {
    let task = WorkNode::new(
        "T1",
        NodeKind::Task,
        "Edit main",
        NodeContract::default(),
        BindingRef::Role("backend".to_string()),
        NodeStatus::Pending,
    );
    let touches: BTreeMap<_, _> =
        BTreeMap::from([("T1".to_string(), BTreeSet::from(["src/main.rs".to_string()]))]);
    let coverage = TouchCoverageReport {
        available: true,
        artifact_languages: BTreeSet::new(),
        touches: touches.clone(),
        unresolved_task_ids: Vec::new(),
    };
    let knowledge = KnowledgeTouchCoverage {
        declared_touches: touches.clone(),
        knowledge_attachment_touches: touches,
        provenance_by_task: BTreeMap::new(),
        resolution_omissions: Vec::new(),
    };
    let mut graph = TaskGraph::new(vec![task], Vec::new());
    derive_project_context_from_knowledge(
        &mut graph,
        &fixture.project,
        Some(&fixture.wiki),
        &coverage,
        &knowledge,
        None,
        KnowledgeAttachmentConfig::production(),
    );
    graph
}

fn kept_pointers(graph: &TaskGraph) -> Vec<String> {
    spawn_context_from_work_graph_task(graph, "T1")
        .task_scope
        .into_iter()
        .filter(|reference| reference.source == KnowledgeSource::Institutional)
        .map(|reference| reference.pointer)
        .collect()
}

fn withheld(graph: &TaskGraph) -> Vec<(String, ContextDropReason)> {
    spawn_context_from_work_graph_task(graph, "T1")
        .withheld
        .into_iter()
        .map(|dropped| (dropped.pointer, dropped.reason))
        .collect()
}

fn omission_details(graph: &TaskGraph) -> Vec<String> {
    graph.omissions.iter().map(|omission| omission.detail.clone()).collect()
}

#[test]
fn eligible_client_page_is_kept_in_its_own_repo() {
    let fixture = fixture("TOG Portal", &["clients/tog.md"], true);
    let graph = derive(&fixture);
    assert_eq!(kept_pointers(&graph), vec!["clients/tog.md"]);
    assert!(withheld(&graph).is_empty());
}

#[test]
fn ineligible_client_page_is_excluded_and_recorded_in_the_sidecar() {
    let fixture = fixture("hive-manager", &["clients/tog.md", "patterns/guide.md"], true);
    let graph = derive(&fixture);

    assert_eq!(kept_pointers(&graph), vec!["patterns/guide.md"]);
    assert_eq!(
        withheld(&graph),
        vec![("clients/tog.md".to_string(), ContextDropReason::ScopeExcluded)]
    );
    // The page was never read into the graph, the miss-sample pool or the prompt.
    let serialized = serde_json::to_string(&graph).unwrap();
    assert!(!serialized.contains(CLIENT_SECRET));
    assert!(knowledge_refs_from_work_graph(&graph)
        .iter()
        .all(|reference| reference.pointer != "clients/tog.md"));
    let composed = compose_context(None, &spawn_context_from_work_graph_task(&graph, "T1"));
    assert!(composed
        .knowledge
        .iter()
        .all(|item| item.reference.pointer != "clients/tog.md"));
    assert!(composed.dropped.iter().any(|dropped| {
        dropped.pointer == "clients/tog.md" && dropped.reason == ContextDropReason::ScopeExcluded
    }));
    let rendered = render_composed_context(&composed);
    assert!(rendered.contains("wiki page scoped to another client"));
    assert!(!rendered.contains(CLIENT_SECRET));
    assert!(omission_details(&graph)
        .contains(&"wiki references scoped to another client were excluded".to_string()));
    assert!(graph.nodes.iter().any(|node| node
        .expansion
        .as_ref()
        .is_some_and(|expansion| expansion.parameters.get(EXCLUDED_CONTEXT_PARAMETER)
            == Some(&"scope_excluded".to_string()))));
}

#[test]
fn oversight_lets_an_office_see_its_managed_client_one_way() {
    let office = derive(&fixture("axiom-coherence", &["clients/egi.md"], true));
    assert_eq!(kept_pointers(&office), vec!["clients/egi.md"]);

    let managed = derive(&fixture("egi-portal", &["clients/axiom-coherence.md", "clients/egi.md"], true));
    assert_eq!(kept_pointers(&managed), vec!["clients/egi.md"]);
    assert_eq!(withheld(&managed)[0].1, ContextDropReason::ScopeExcluded);
}

#[test]
fn universal_pages_are_unaffected() {
    let graph = derive(&fixture("hive-manager", &["patterns/guide.md"], true));
    assert_eq!(kept_pointers(&graph), vec!["patterns/guide.md"]);
    let summary = spawn_context_from_work_graph_task(&graph, "T1")
        .task_scope
        .into_iter()
        .find(|reference| reference.pointer == "patterns/guide.md")
        .and_then(|reference| reference.summary);
    assert_eq!(summary.as_deref(), Some("Reliable planning: Keep evidence attached."));
    assert!(withheld(&graph).is_empty());
}

#[test]
fn dangling_and_unparseable_refs_are_recorded_not_silently_dropped() {
    let graph = derive(&fixture(
        "hive-manager",
        &["patterns/missing.md", "notes/not-a-category.md"],
        true,
    ));
    assert!(kept_pointers(&graph).is_empty());
    assert_eq!(
        withheld(&graph),
        vec![
            ("notes/not-a-category.md".to_string(), ContextDropReason::UnresolvedGlobalRef),
            ("patterns/missing.md".to_string(), ContextDropReason::UnresolvedGlobalRef),
        ]
    );
    assert!(omission_details(&graph)
        .contains(&"wiki references did not resolve to a wiki page".to_string()));
}

#[test]
fn stale_map_still_isolates_an_entity_page_by_path() {
    // clients/new-client.md is absent from the fixture map's pages{}.
    let graph = derive(&fixture("hive-manager", &["clients/new-client.md"], true));
    assert_eq!(
        withheld(&graph),
        vec![("clients/new-client.md".to_string(), ContextDropReason::ScopeExcluded)]
    );
}

#[test]
fn frontmatter_scope_covers_a_page_added_after_the_map() {
    let graph = derive(&fixture("hive-manager", &["practices/added-after-map.md"], true));
    assert_eq!(kept_pointers(&graph), vec!["practices/added-after-map.md"]);
}

#[test]
fn a_page_with_no_known_scope_is_excluded_as_unknown() {
    let graph = derive(&fixture("hive-manager", &["practices/no-scope.md"], true));
    assert!(kept_pointers(&graph).is_empty());
    assert_eq!(
        withheld(&graph),
        vec![("practices/no-scope.md".to_string(), ContextDropReason::ScopeUnknown)]
    );
}

#[test]
fn missing_map_fails_closed_on_clients_and_open_on_universal_frontmatter() {
    let fixture = fixture(
        "TOG Portal",
        &["clients/tog.md", "patterns/guide.md", "practices/no-scope.md"],
        false,
    );
    let graph = derive(&fixture);
    // Even the client's own repo loses its client pages: no map, no repos table.
    assert_eq!(kept_pointers(&graph), vec!["patterns/guide.md"]);
    assert_eq!(
        withheld(&graph),
        vec![
            ("clients/tog.md".to_string(), ContextDropReason::ScopeExcluded),
            ("practices/no-scope.md".to_string(), ContextDropReason::ScopeUnknown),
        ]
    );
    let details = omission_details(&graph);
    assert_eq!(
        details
            .iter()
            .filter(|detail| detail.starts_with("wiki label map was unavailable"))
            .count(),
        1
    );
    assert!(!serde_json::to_string(&graph).unwrap().contains(CLIENT_SECRET));
}

#[test]
fn derivation_is_deterministic() {
    let fixture = fixture(
        "hive-manager",
        &["patterns/missing.md", "clients/tog.md", "patterns/guide.md", "practices/no-scope.md"],
        true,
    );
    let first = serde_json::to_string(&derive(&fixture)).unwrap();
    let second = serde_json::to_string(&derive(&fixture)).unwrap();
    assert_eq!(first, second);
    let spawn: SpawnContext = spawn_context_from_work_graph_task(&derive(&fixture), "T1");
    let pointers: Vec<_> = spawn.withheld.iter().map(|dropped| dropped.pointer.as_str()).collect();
    let mut sorted = pointers.clone();
    sorted.sort();
    assert_eq!(pointers, sorted);
}
