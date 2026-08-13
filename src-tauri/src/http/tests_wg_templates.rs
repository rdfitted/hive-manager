//! Graph-archetype tests for issue #216, owned by WS-7.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::orchestrator::work_graph::archetypes::{
    built_in_archetypes, instantiate_named_archetype, propose_deviation_promotions,
    ArchetypeLane, ArchetypeSource, DeviationObservation, GotchaAttachment,
    GotchaAttachmentProvider, GraphArchetype, GraphArchetypeCatalog,
    GraphArchetypeOverride, GraphOverrideCatalog, NoGotchaAttachments,
    NoRepoShapeFacts, PromotionTier, RepoShapeFacts, RepoShapeFactsProvider,
    INSTITUTIONAL_ARCHETYPE_CATALOG, PROJECT_ARCHETYPE_OVERRIDES,
};
use crate::orchestrator::work_graph::{
    BindingRef, NodeContract, NodeKind, WorkGraphOmissionReason,
};

#[derive(Clone)]
struct StaticFacts(&'static [&'static str]);

impl RepoShapeFactsProvider for StaticFacts {
    fn facts(&self, _project_path: &Path) -> Result<Option<RepoShapeFacts>, String> {
        Ok(Some(RepoShapeFacts {
            facts: self.0.iter().map(|fact| (*fact).to_string()).collect(),
        }))
    }
}

#[derive(Clone)]
struct StaticGotchas(Vec<GotchaAttachment>);

impl GotchaAttachmentProvider for StaticGotchas {
    fn gotchas(&self, _project_path: &Path) -> Result<Option<Vec<GotchaAttachment>>, String> {
        Ok(Some(self.0.clone()))
    }
}

#[test]
fn named_archetype_applies_repo_shape_overrides_gotchas_and_lineage() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    let wiki = fixture.path().join("wiki");
    fs::create_dir_all(project.join(".ai-docs")).unwrap();
    fs::create_dir_all(wiki.join("tools")).unwrap();

    let mut archetype = feature_build();
    archetype.version = 7;
    write_json(
        &wiki.join(INSTITUTIONAL_ARCHETYPE_CATALOG),
        &GraphArchetypeCatalog {
            version: 3,
            archetypes: vec![archetype],
        },
    );
    write_json(
        &project.join(".ai-docs").join(PROJECT_ARCHETYPE_OVERRIDES),
        &GraphOverrideCatalog {
            version: 1,
            overrides: vec![GraphArchetypeOverride {
                id: "backend-only".to_string(),
                archetype_id: "feature-build".to_string(),
                remove_lanes: vec!["integrate".to_string()],
                add_lanes: vec![lane(
                    "package",
                    "Package ${component}",
                    "artifact",
                    &["backend"],
                )],
                parameters: BTreeMap::from([(
                    "component".to_string(),
                    "billing".to_string(),
                )]),
            }],
        },
    );

    let instance = instantiate_named_archetype(
        &project,
        Some(&wiki),
        "feature-build",
        &BTreeMap::new(),
        &StaticFacts(&["backend"]),
        &StaticGotchas(vec![GotchaAttachment {
            lane_id: "backend".to_string(),
            acceptance: "preserve idempotency".to_string(),
        }]),
    )
    .unwrap();

    assert_eq!(instance.lineage.template_id, "feature-build");
    assert_eq!(instance.lineage.template_version, 7);
    assert_eq!(instance.lineage.source, ArchetypeSource::InstitutionalCatalog);
    assert_eq!(instance.lineage.applied_override_ids, vec!["backend-only"]);
    assert!(has_node(&instance.graph, "backend"));
    assert!(has_node(&instance.graph, "package"));
    assert!(!has_node(&instance.graph, "frontend"), "repo facts prune frontend");
    assert!(!has_node(&instance.graph, "integrate"), "override prunes integrate");
    assert_eq!(
        instance
            .graph
            .nodes
            .iter()
            .find(|node| node.id == "backend")
            .unwrap()
            .title,
        "Build billing backend"
    );
    assert!(instance
        .graph
        .nodes
        .iter()
        .find(|node| node.id == "backend")
        .unwrap()
        .contract
        .acceptance
        .contains(&"preserve idempotency".to_string()));
    assert!(instance
        .graph
        .nodes
        .iter()
        .any(|node| node.id.starts_with("backend::review::")));

    let json = serde_json::to_string(&instance).unwrap();
    let decoded = serde_json::from_str(&json).unwrap();
    assert_eq!(instance, decoded, "lineage survives persistence round-trip");
}

#[test]
fn missing_overrides_and_codegraph_preserve_vanilla_and_report_absence() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    let wiki = fixture.path().join("wiki");
    fs::create_dir_all(project.join(".ai-docs")).unwrap();
    fs::create_dir_all(&wiki).unwrap();

    let without_facts = instantiate_named_archetype(
        &project,
        Some(&wiki),
        "feature-build",
        &BTreeMap::new(),
        &NoRepoShapeFacts,
        &NoGotchaAttachments,
    )
    .unwrap();
    let with_all_facts = instantiate_named_archetype(
        &project,
        Some(&wiki),
        "feature-build",
        &BTreeMap::new(),
        &StaticFacts(&["backend", "frontend"]),
        &StaticGotchas(Vec::new()),
    )
    .unwrap();

    assert_eq!(without_facts.graph.nodes, with_all_facts.graph.nodes);
    assert_eq!(without_facts.graph.edges, with_all_facts.graph.edges);
    assert!(without_facts.graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::CodegraphUnavailable
            && omission.examples == vec!["repo-shape-facts"]
    }));
    assert!(without_facts.graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::ProjectKnowledgeUnavailable
            && omission.examples == vec!["gotcha-attachments"]
    }));
    assert!(without_facts.graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::SourceUnreadable
            && omission
                .examples
                .iter()
                .any(|example| example.contains(INSTITUTIONAL_ARCHETYPE_CATALOG))
    }));
}

#[test]
fn project_override_changes_instantiated_topology() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    fs::create_dir_all(project.join(".ai-docs")).unwrap();
    write_json(
        &project.join(".ai-docs").join(PROJECT_ARCHETYPE_OVERRIDES),
        &GraphOverrideCatalog {
            version: 1,
            overrides: vec![GraphArchetypeOverride {
                id: "no-frontend".to_string(),
                archetype_id: "feature-build".to_string(),
                remove_lanes: vec!["frontend".to_string()],
                add_lanes: Vec::new(),
                parameters: BTreeMap::new(),
            }],
        },
    );

    let instance = instantiate_named_archetype(
        &project,
        None,
        "feature-build",
        &BTreeMap::new(),
        &NoRepoShapeFacts,
        &StaticGotchas(Vec::new()),
    )
    .unwrap();

    assert!(!has_node(&instance.graph, "frontend"));
    assert_eq!(instance.lineage.applied_override_ids, vec!["no-frontend"]);
}

#[test]
fn repeated_deviations_emit_proposals_without_writing_project_or_wiki() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    let wiki = fixture.path().join("wiki");
    fs::create_dir_all(project.join(".ai-docs")).unwrap();
    fs::create_dir_all(wiki.join("tools")).unwrap();
    fs::write(project.join(".ai-docs").join("sentinel"), b"project").unwrap();
    fs::write(wiki.join("tools").join("sentinel"), b"wiki").unwrap();
    let before_project = snapshot_tree(&project);
    let before_wiki = snapshot_tree(&wiki);

    let proposals = propose_deviation_promotions(&[
        observation("repo-a", "no-frontend"),
        observation("repo-a", "no-frontend"),
        observation("repo-a", "always-contract-test"),
        observation("repo-b", "always-contract-test"),
    ]);

    assert!(proposals.iter().any(|proposal| {
        proposal.tier == PromotionTier::ProjectOverride
            && proposal.deviation_key == "no-frontend"
            && proposal.observation_count == 2
    }));
    assert!(proposals.iter().any(|proposal| {
        proposal.tier == PromotionTier::InstitutionalRevision
            && proposal.deviation_key == "always-contract-test"
            && proposal.repo_ids == vec!["repo-a", "repo-b"]
    }));
    assert_eq!(snapshot_tree(&project), before_project);
    assert_eq!(snapshot_tree(&wiki), before_wiki);
}

#[test]
fn project_path_reads_overrides_when_worker_worktree_has_no_ai_docs() {
    let fixture = TempDir::new().unwrap();
    let project = fixture.path().join("project");
    let worktree = project
        .join(".hive-manager")
        .join("worktrees")
        .join("session")
        .join("primary");
    fs::create_dir_all(project.join(".ai-docs")).unwrap();
    fs::create_dir_all(&worktree).unwrap();
    write_json(
        &project.join(".ai-docs").join(PROJECT_ARCHETYPE_OVERRIDES),
        &GraphOverrideCatalog {
            version: 1,
            overrides: vec![GraphArchetypeOverride {
                id: "root-owned-override".to_string(),
                archetype_id: "feature-build".to_string(),
                remove_lanes: vec!["frontend".to_string()],
                add_lanes: Vec::new(),
                parameters: BTreeMap::new(),
            }],
        },
    );
    assert!(!worktree.join(".ai-docs").exists());

    let instance = instantiate_named_archetype(
        &project,
        None,
        "feature-build",
        &BTreeMap::new(),
        &NoRepoShapeFacts,
        &StaticGotchas(Vec::new()),
    )
    .unwrap();

    assert!(!has_node(&instance.graph, "frontend"));
    assert_eq!(
        instance.lineage.applied_override_ids,
        vec!["root-owned-override"]
    );
    assert!(!worktree.join(".ai-docs").exists());
}

#[test]
fn registry_exposes_four_distinct_named_archetypes() {
    let archetypes = built_in_archetypes();
    let ids: BTreeSet<_> = archetypes.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, BTreeSet::from(["audit", "bug-hunt", "feature-build", "migration"]));
    assert!(archetypes.iter().all(|item| item.version > 0));
}

fn feature_build() -> GraphArchetype {
    built_in_archetypes()
        .into_iter()
        .find(|item| item.id == "feature-build")
        .unwrap()
}

fn lane(id: &str, title: &str, output: &str, depends_on: &[&str]) -> ArchetypeLane {
    ArchetypeLane {
        id: id.to_string(),
        title: title.to_string(),
        kind: NodeKind::Task,
        contract: NodeContract {
            inputs: Vec::new(),
            outputs: vec![output.to_string()],
            acceptance: vec![format!("{id} accepted")],
        },
        binding: BindingRef::Role("backend".to_string()),
        depends_on: depends_on.iter().map(|id| (*id).to_string()).collect(),
        required_repo_facts: Vec::new(),
    }
}

fn has_node(graph: &crate::orchestrator::work_graph::TaskGraph, id: &str) -> bool {
    graph.nodes.iter().any(|node| node.id == id)
}

fn observation(repo_id: &str, deviation_key: &str) -> DeviationObservation {
    DeviationObservation {
        repo_id: repo_id.to_string(),
        archetype_id: "feature-build".to_string(),
        deviation_key: deviation_key.to_string(),
        detail: format!("observed {deviation_key}"),
    }
}

fn write_json(path: &Path, value: &impl serde::Serialize) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, current: &Path, snapshot: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(current)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, snapshot);
            } else {
                snapshot.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                ));
            }
        }
    }

    let mut snapshot = Vec::new();
    visit(root, root, &mut snapshot);
    snapshot
}
