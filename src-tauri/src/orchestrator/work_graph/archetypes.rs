//! Versioned work-graph archetypes for issue #216.
//!
//! This module is intentionally separate from `templates::SessionTemplate`.
//! Session templates describe the roster; graph archetypes describe the work
//! topology that roster executes.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::review::{
    instantiate_checkpoint_wave, instantiate_review_templates, CheckpointWave, ReviewGraphError,
    ReviewTemplate,
};
use super::schema::TaskTier;
use super::{
    BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus,
    TaskGraph, TaskId, WorkEdge, WorkGraphOmission, WorkGraphOmissionReason, WorkNode,
};

pub const INSTITUTIONAL_ARCHETYPE_CATALOG: &str = "tools/work-graph-archetypes.json";
pub const PROJECT_ARCHETYPE_OVERRIDES: &str = "work-graph-overrides.json";
pub const GRAPH_ARCHETYPE_EXPANSION: &str = "graph-archetype";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphArchetypeCatalog {
    pub version: u32,
    #[serde(default)]
    pub archetypes: Vec<GraphArchetype>,
}

impl GraphArchetypeCatalog {
    pub fn built_in() -> Self {
        Self {
            version: 1,
            archetypes: built_in_archetypes(),
        }
    }
}

/// A versioned topology skeleton. Literal `${name}` tokens are holes filled at
/// instantiation from defaults, project overrides, then session parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphArchetype {
    pub id: String,
    pub version: u32,
    pub description: String,
    #[serde(default)]
    pub holes: Vec<String>,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    #[serde(default)]
    pub lanes: Vec<ArchetypeLane>,
    #[serde(default)]
    pub review_templates: Vec<ReviewTemplate>,
    #[serde(default)]
    pub checkpoints: Vec<CheckpointWave>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchetypeLane {
    pub id: TaskId,
    pub title: String,
    pub kind: NodeKind,
    pub contract: NodeContract,
    pub binding: BindingRef,
    #[serde(default)]
    pub depends_on: Vec<TaskId>,
    /// A lane is pruned only when codegraph facts are available and omit one
    /// of these values. Unavailable codegraph data preserves the vanilla lane
    /// and emits an omission instead of pretending it was inapplicable.
    #[serde(default)]
    pub required_repo_facts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GraphOverrideCatalog {
    pub version: u32,
    #[serde(default)]
    pub overrides: Vec<GraphArchetypeOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphArchetypeOverride {
    pub id: String,
    pub archetype_id: String,
    #[serde(default)]
    pub remove_lanes: Vec<TaskId>,
    #[serde(default)]
    pub add_lanes: Vec<ArchetypeLane>,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
}

/// Published lineage interface consumed by graph retrospectives (#217).
/// The field on [`GraphArchetypeInstance`] is named `lineage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchetypeLineage {
    pub template_id: String,
    pub template_version: u32,
    pub source: ArchetypeSource,
    #[serde(default)]
    pub applied_override_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchetypeSource {
    InstitutionalCatalog,
    EmbeddedDefault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphArchetypeInstance {
    pub graph: TaskGraph,
    pub lineage: ArchetypeLineage,
}

/// Skeleton-stage output. Reviews and checkpoints are deliberately returned
/// unstamped so touch/context enrichment can run before those plan-time stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchetypeCompositionStage {
    pub instance: GraphArchetypeInstance,
    pub review_templates: Vec<ReviewTemplate>,
    pub checkpoints: Vec<CheckpointWave>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoShapeFacts {
    pub facts: BTreeSet<String>,
}

/// Narrow seam for Wave 3 codegraph facts (#215).
pub trait RepoShapeFactsProvider {
    fn facts(&self, project_path: &Path) -> Result<Option<RepoShapeFacts>, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoRepoShapeFacts;

impl RepoShapeFactsProvider for NoRepoShapeFacts {
    fn facts(&self, _project_path: &Path) -> Result<Option<RepoShapeFacts>, String> {
        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GotchaAttachment {
    pub lane_id: TaskId,
    pub acceptance: String,
}

/// Narrow seam for project-knowledge gotcha attachment (#218).
pub trait GotchaAttachmentProvider {
    fn gotchas(&self, project_path: &Path) -> Result<Option<Vec<GotchaAttachment>>, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoGotchaAttachments;

impl GotchaAttachmentProvider for NoGotchaAttachments {
    fn gotchas(&self, _project_path: &Path) -> Result<Option<Vec<GotchaAttachment>>, String> {
        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchetypeError {
    UnknownArchetype(String),
    MissingParameter(String),
    DuplicateLane(TaskId),
    Review(String),
}

impl fmt::Display for ArchetypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownArchetype(id) => write!(formatter, "unknown graph archetype {id}"),
            Self::MissingParameter(name) => {
                write!(formatter, "graph archetype parameter {name} has no value")
            }
            Self::DuplicateLane(id) => write!(formatter, "graph archetype repeats lane {id}"),
            Self::Review(error) => write!(formatter, "review expansion failed: {error}"),
        }
    }
}

impl Error for ArchetypeError {}

impl From<ReviewGraphError> for ArchetypeError {
    fn from(error: ReviewGraphError) -> Self {
        Self::Review(error.to_string())
    }
}

/// Instantiate a named topology using read-only institutional and project
/// sources. `project_path` must be the session's project root, never its worker
/// CWD/worktree. The institutional root is already resolved by the caller's
/// configuration boundary.
pub fn instantiate_named_archetype<R, G>(
    project_path: &Path,
    institutional_wiki_root: Option<&Path>,
    archetype_id: &str,
    session_parameters: &BTreeMap<String, String>,
    repo_shape: &R,
    gotcha_provider: &G,
) -> Result<GraphArchetypeInstance, ArchetypeError>
where
    R: RepoShapeFactsProvider,
    G: GotchaAttachmentProvider,
{
    let mut prepared = prepare_named_archetype(
        project_path,
        institutional_wiki_root,
        archetype_id,
        session_parameters,
        repo_shape,
    )?;
    match gotcha_provider.gotchas(project_path) {
        Ok(Some(gotchas)) => {
            attach_gotchas(&mut prepared.archetype, gotchas, &mut prepared.omissions)
        }
        Ok(None) => prepared.omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::ProjectKnowledgeUnavailable,
            1,
            vec!["gotcha-attachments".to_string()],
        )),
        Err(error) => prepared.omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::SourceUnreadable,
            1,
            vec![format!("gotcha-attachments: {error}")],
        )),
    }
    let mut graph = build_lane_graph(
        &prepared.archetype,
        &prepared.lineage,
        &mut prepared.omissions,
    )?;
    instantiate_review_templates(&mut graph, &prepared.archetype.review_templates)?;
    instantiate_checkpoints(
        &mut graph,
        &prepared.archetype.checkpoints,
        &mut prepared.omissions,
    )?;
    graph.omissions.extend(prepared.omissions);
    Ok(GraphArchetypeInstance {
        graph,
        lineage: prepared.lineage,
    })
}

/// Compose a selected/pruned/overridden/parameterized skeleton onto the graph
/// already under construction. No context clone and no separate graph escapes.
pub fn instantiate_named_archetype_into<R>(
    mut graph: TaskGraph,
    project_path: &Path,
    institutional_wiki_root: Option<&Path>,
    archetype_id: &str,
    session_parameters: &BTreeMap<String, String>,
    repo_shape: &R,
) -> Result<ArchetypeCompositionStage, ArchetypeError>
where
    R: RepoShapeFactsProvider,
{
    let mut prepared = prepare_named_archetype(
        project_path,
        institutional_wiki_root,
        archetype_id,
        session_parameters,
        repo_shape,
    )?;
    let skeleton = build_lane_graph(
        &prepared.archetype,
        &prepared.lineage,
        &mut prepared.omissions,
    )?;
    let mut known: BTreeSet<_> = graph.nodes.iter().map(|node| node.id.clone()).collect();
    for node in skeleton.nodes {
        if !known.insert(node.id.clone()) {
            return Err(ArchetypeError::DuplicateLane(node.id));
        }
        graph.nodes.push(node);
    }
    for edge in skeleton.edges {
        if !graph.edges.contains(&edge) {
            graph.edges.push(edge);
        }
    }
    graph.omissions.extend(prepared.omissions);
    Ok(ArchetypeCompositionStage {
        instance: GraphArchetypeInstance {
            graph,
            lineage: prepared.lineage,
        },
        review_templates: prepared.archetype.review_templates,
        checkpoints: prepared.archetype.checkpoints,
    })
}

struct PreparedArchetype {
    archetype: GraphArchetype,
    lineage: ArchetypeLineage,
    omissions: Vec<WorkGraphOmission>,
}

fn prepare_named_archetype<R>(
    project_path: &Path,
    institutional_wiki_root: Option<&Path>,
    archetype_id: &str,
    session_parameters: &BTreeMap<String, String>,
    repo_shape: &R,
) -> Result<PreparedArchetype, ArchetypeError>
where
    R: RepoShapeFactsProvider,
{
    let (catalog, catalog_source, mut omissions) =
        load_institutional_catalog(institutional_wiki_root);
    let institutional_match = catalog
        .archetypes
        .iter()
        .find(|candidate| candidate.id == archetype_id)
        .cloned();
    let (mut archetype, source) = institutional_match
        .map(|archetype| (archetype, catalog_source))
        .or_else(|| {
            GraphArchetypeCatalog::built_in()
                .archetypes
                .into_iter()
                .find(|candidate| candidate.id == archetype_id)
                .map(|archetype| (archetype, ArchetypeSource::EmbeddedDefault))
        })
        .ok_or_else(|| ArchetypeError::UnknownArchetype(archetype_id.to_string()))?;
    if source == ArchetypeSource::EmbeddedDefault
        && catalog_source == ArchetypeSource::InstitutionalCatalog
    {
        omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::ResolutionIncomplete,
            1,
            vec![format!(
                "institutional catalog omitted archetype {archetype_id}; used embedded default"
            )],
        ));
    }

    let (override_catalog, override_omission) = load_project_overrides(project_path);
    if let Some(omission) = override_omission {
        omissions.push(omission);
    }
    let matching_overrides: Vec<_> = override_catalog
        .overrides
        .into_iter()
        .filter(|candidate| candidate.archetype_id == archetype.id)
        .collect();
    match repo_shape.facts(project_path) {
        Ok(Some(facts)) => {
            archetype.lanes.retain(|lane| {
                lane.required_repo_facts
                    .iter()
                    .all(|fact| facts.facts.contains(fact))
            });
        }
        Ok(None) => omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::CodegraphUnavailable,
            1,
            vec!["repo-shape-facts".to_string()],
        )),
        Err(error) => omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::SourceUnreadable,
            1,
            vec![format!("repo-shape-facts: {error}")],
        )),
    }

    // Repo-shape pruning narrows the institutional skeleton first. Standing
    // project overrides then win deliberately: they may remove an irrelevant
    // lane or add a repo-specific lane the generic shape could not infer.
    let mut applied_override_ids = Vec::new();
    for project_override in &matching_overrides {
        apply_override(&mut archetype, project_override)?;
        applied_override_ids.push(project_override.id.clone());
    }
    applied_override_ids.sort();

    for (name, value) in session_parameters {
        archetype.parameters.insert(name.clone(), value.clone());
    }
    for hole in &archetype.holes {
        if !archetype.parameters.contains_key(hole) {
            return Err(ArchetypeError::MissingParameter(hole.clone()));
        }
    }

    fill_archetype_holes(&mut archetype);
    let lineage = ArchetypeLineage {
        template_id: archetype.id.clone(),
        template_version: archetype.version,
        source,
        applied_override_ids,
    };
    Ok(PreparedArchetype {
        archetype,
        lineage,
        omissions,
    })
}

/// Deterministically reconcile planner output onto the persisted enriched
/// skeleton. Stable skeleton/derived nodes, lineage, and omissions survive;
/// retrying the same planner graph is an exact no-op.
pub fn reconcile_planner_graph(
    persisted: &TaskGraph,
    planner_graph: &TaskGraph,
) -> Result<TaskGraph, ArchetypeError> {
    let mut reconciled = persisted.clone();
    let mut planner_nodes = planner_graph.nodes.clone();
    planner_nodes.sort_by(|left, right| left.id.cmp(&right.id));
    let mut planner_ids = BTreeSet::new();
    for planner_node in planner_nodes {
        if !planner_ids.insert(planner_node.id.clone()) {
            return Err(ArchetypeError::DuplicateLane(planner_node.id));
        }
        if let Some(existing) = reconciled
            .nodes
            .iter_mut()
            .find(|node| node.id == planner_node.id)
        {
            existing.title = planner_node.title;
            existing.contract = planner_node.contract;
            existing.binding = planner_node.binding;
            if planner_node.status == NodeStatus::Completed {
                existing.status = NodeStatus::Completed;
            }
        } else {
            reconciled.nodes.push(planner_node);
        }
    }
    for edge in &planner_graph.edges {
        if !reconciled.edges.contains(edge) {
            reconciled.edges.push(edge.clone());
        }
    }
    for omission in &planner_graph.omissions {
        if !reconciled.omissions.contains(omission) {
            reconciled.omissions.push(omission.clone());
        }
    }
    reconciled
        .nodes
        .sort_by(|left, right| left.id.cmp(&right.id));
    reconciled.edges.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then(left.target.cmp(&right.target))
            .then(format!("{:?}", left.kind).cmp(&format!("{:?}", right.kind)))
            .then(format!("{:?}", left.provenance).cmp(&format!("{:?}", right.provenance)))
            .then(left.rationale.cmp(&right.rationale))
    });
    Ok(reconciled)
}

/// Stamp opt-in checkpoint declarations after touch/context enrichment.
pub fn stamp_checkpoint_waves(
    graph: &mut TaskGraph,
    checkpoints: &[CheckpointWave],
) -> Result<(), ArchetypeError> {
    let mut omissions = Vec::new();
    instantiate_checkpoints(graph, checkpoints, &mut omissions)?;
    graph.omissions.extend(omissions);
    Ok(())
}

fn load_institutional_catalog(
    institutional_wiki_root: Option<&Path>,
) -> (
    GraphArchetypeCatalog,
    ArchetypeSource,
    Vec<WorkGraphOmission>,
) {
    let fallback = || {
        (
            GraphArchetypeCatalog::built_in(),
            ArchetypeSource::EmbeddedDefault,
        )
    };
    let Some(root) = institutional_wiki_root else {
        let (catalog, source) = fallback();
        return (
            catalog,
            source,
            vec![WorkGraphOmission::new(
                WorkGraphOmissionReason::SourceUnreadable,
                1,
                vec!["institutional-work-graph-catalog: root unavailable".to_string()],
            )],
        );
    };
    let path = root.join(INSTITUTIONAL_ARCHETYPE_CATALOG);
    match fs::read_to_string(&path) {
        Ok(json) => match serde_json::from_str(&json) {
            Ok(catalog) => (catalog, ArchetypeSource::InstitutionalCatalog, Vec::new()),
            Err(error) => {
                let (catalog, source) = fallback();
                (
                    catalog,
                    source,
                    vec![WorkGraphOmission::new(
                        WorkGraphOmissionReason::SourceUnreadable,
                        1,
                        vec![format!("{}: {error}", path.display())],
                    )],
                )
            }
        },
        Err(error) => {
            let (catalog, source) = fallback();
            (
                catalog,
                source,
                vec![WorkGraphOmission::new(
                    WorkGraphOmissionReason::SourceUnreadable,
                    1,
                    vec![format!("{}: {error}", path.display())],
                )],
            )
        }
    }
}

fn load_project_overrides(
    project_path: &Path,
) -> (GraphOverrideCatalog, Option<WorkGraphOmission>) {
    let ai_docs = project_path.join(".ai-docs");
    if !ai_docs.is_dir() {
        return (
            GraphOverrideCatalog::default(),
            Some(WorkGraphOmission::new(
                WorkGraphOmissionReason::ProjectKnowledgeUnavailable,
                1,
                vec![ai_docs.display().to_string()],
            )),
        );
    }
    let path = ai_docs.join(PROJECT_ARCHETYPE_OVERRIDES);
    if !path.exists() {
        return (GraphOverrideCatalog::default(), None);
    }
    match fs::read_to_string(&path) {
        Ok(json) => match serde_json::from_str(&json) {
            Ok(catalog) => (catalog, None),
            Err(error) => (
                GraphOverrideCatalog::default(),
                Some(WorkGraphOmission::new(
                    WorkGraphOmissionReason::SourceUnreadable,
                    1,
                    vec![format!("{}: {error}", path.display())],
                )),
            ),
        },
        Err(error) => (
            GraphOverrideCatalog::default(),
            Some(WorkGraphOmission::new(
                WorkGraphOmissionReason::SourceUnreadable,
                1,
                vec![format!("{}: {error}", path.display())],
            )),
        ),
    }
}

fn apply_override(
    archetype: &mut GraphArchetype,
    project_override: &GraphArchetypeOverride,
) -> Result<(), ArchetypeError> {
    let removed: BTreeSet<_> = project_override.remove_lanes.iter().collect();
    archetype.lanes.retain(|lane| !removed.contains(&lane.id));
    archetype.lanes.extend(project_override.add_lanes.clone());
    for (name, value) in &project_override.parameters {
        archetype.parameters.insert(name.clone(), value.clone());
    }
    ensure_unique_lanes(&archetype.lanes)
}

fn ensure_unique_lanes(lanes: &[ArchetypeLane]) -> Result<(), ArchetypeError> {
    let mut ids = BTreeSet::new();
    for lane in lanes {
        if !ids.insert(&lane.id) {
            return Err(ArchetypeError::DuplicateLane(lane.id.clone()));
        }
    }
    Ok(())
}

fn attach_gotchas(
    archetype: &mut GraphArchetype,
    gotchas: Vec<GotchaAttachment>,
    omissions: &mut Vec<WorkGraphOmission>,
) {
    for gotcha in gotchas {
        match archetype
            .lanes
            .iter_mut()
            .find(|lane| lane.id == gotcha.lane_id)
        {
            Some(lane) => lane.contract.acceptance.push(gotcha.acceptance),
            None => omissions.push(WorkGraphOmission::new(
                WorkGraphOmissionReason::ResolutionIncomplete,
                1,
                vec![format!("gotcha lane {}", gotcha.lane_id)],
            )),
        }
    }
}

fn fill_archetype_holes(archetype: &mut GraphArchetype) {
    for lane in &mut archetype.lanes {
        lane.id = fill(&lane.id, &archetype.parameters);
        lane.title = fill(&lane.title, &archetype.parameters);
        fill_contract(&mut lane.contract, &archetype.parameters);
        fill_binding(&mut lane.binding, &archetype.parameters);
        for dependency in &mut lane.depends_on {
            *dependency = fill(dependency, &archetype.parameters);
        }
        for fact in &mut lane.required_repo_facts {
            *fact = fill(fact, &archetype.parameters);
        }
    }
    for review in &mut archetype.review_templates {
        review.id = fill(&review.id, &archetype.parameters);
        if let Some(required_output) = &mut review.required_output {
            *required_output = fill(required_output, &archetype.parameters);
        }
        for lens in &mut review.lenses {
            lens.id = fill(&lens.id, &archetype.parameters);
            lens.focus = fill(&lens.focus, &archetype.parameters);
            for acceptance in &mut lens.acceptance {
                *acceptance = fill(acceptance, &archetype.parameters);
            }
            fill_binding(&mut lens.binding, &archetype.parameters);
        }
        for rubric in &mut review.rubric {
            *rubric = fill(rubric, &archetype.parameters);
        }
        fill_binding(&mut review.verdict_binding, &archetype.parameters);
        fill_binding(&mut review.remediation_binding, &archetype.parameters);
    }
    for checkpoint in &mut archetype.checkpoints {
        checkpoint.id = fill(&checkpoint.id, &archetype.parameters);
        checkpoint.title = fill(&checkpoint.title, &archetype.parameters);
        for id in &mut checkpoint.prerequisites {
            *id = fill(id, &archetype.parameters);
        }
        for id in &mut checkpoint.downstream {
            *id = fill(id, &archetype.parameters);
        }
        fill_binding(&mut checkpoint.binding, &archetype.parameters);
        for acceptance in &mut checkpoint.acceptance {
            *acceptance = fill(acceptance, &archetype.parameters);
        }
    }
}

fn fill_contract(contract: &mut NodeContract, parameters: &BTreeMap<String, String>) {
    for value in contract
        .inputs
        .iter_mut()
        .chain(&mut contract.outputs)
        .chain(&mut contract.acceptance)
    {
        *value = fill(value, parameters);
    }
}

fn fill_binding(binding: &mut BindingRef, parameters: &BTreeMap<String, String>) {
    match binding {
        BindingRef::Role(value) | BindingRef::Zone(value) => {
            *value = fill(value, parameters);
        }
    }
}

fn fill(value: &str, parameters: &BTreeMap<String, String>) -> String {
    parameters
        .iter()
        .fold(value.to_string(), |rendered, (name, value)| {
            rendered.replace(&format!("${{{name}}}"), value)
        })
}

fn build_lane_graph(
    archetype: &GraphArchetype,
    lineage: &ArchetypeLineage,
    omissions: &mut Vec<WorkGraphOmission>,
) -> Result<TaskGraph, ArchetypeError> {
    ensure_unique_lanes(&archetype.lanes)?;
    let lane_ids: BTreeSet<_> = archetype.lanes.iter().map(|lane| lane.id.clone()).collect();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for lane in &archetype.lanes {
        let mut node = WorkNode::new(
            &lane.id,
            lane.kind,
            &lane.title,
            lane.contract.clone(),
            lane.binding.clone(),
            NodeStatus::Pending,
        );
        let mut parameters = BTreeMap::new();
        parameters.insert("template_id".to_string(), lineage.template_id.clone());
        parameters.insert(
            "template_version".to_string(),
            lineage.template_version.to_string(),
        );
        parameters.insert("lane_id".to_string(), lane.id.clone());
        node.expansion = Some(CompositeExpansion {
            template: GRAPH_ARCHETYPE_EXPANSION.to_string(),
            parameters,
        });
        nodes.push(node);
        for dependency in &lane.depends_on {
            if lane_ids.contains(dependency) {
                edges.push(
                    WorkEdge::new(
                        dependency,
                        &lane.id,
                        EdgeKind::DependsOn,
                        EdgeProvenance::Planner,
                    )
                    .with_rationale(format!(
                        "{} follows the {} archetype lane {}",
                        lane.id, archetype.id, dependency
                    )),
                );
            } else {
                omissions.push(WorkGraphOmission::new(
                    WorkGraphOmissionReason::ResolutionIncomplete,
                    1,
                    vec![format!("{} depends on pruned lane {dependency}", lane.id)],
                ));
            }
        }
    }
    Ok(TaskGraph::new(nodes, edges))
}

fn instantiate_checkpoints(
    graph: &mut TaskGraph,
    checkpoints: &[CheckpointWave],
    omissions: &mut Vec<WorkGraphOmission>,
) -> Result<(), ArchetypeError> {
    for declaration in checkpoints {
        let known: BTreeSet<_> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        let mut checkpoint = declaration.clone();
        let unresolved: Vec<_> = checkpoint
            .prerequisites
            .iter()
            .chain(&checkpoint.downstream)
            .filter(|id| !known.contains(*id))
            .cloned()
            .collect();
        if !unresolved.is_empty() {
            omissions.push(WorkGraphOmission::new(
                WorkGraphOmissionReason::ResolutionIncomplete,
                unresolved.len(),
                unresolved,
            ));
        }
        checkpoint.prerequisites.retain(|id| known.contains(id));
        checkpoint.downstream.retain(|id| known.contains(id));
        if checkpoint.prerequisites.is_empty() || checkpoint.downstream.is_empty() {
            continue;
        }
        instantiate_checkpoint_wave(graph, &checkpoint)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviationObservation {
    pub repo_id: String,
    pub archetype_id: String,
    pub deviation_key: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionTier {
    ProjectOverride,
    InstitutionalRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviationPromotionProposal {
    pub tier: PromotionTier,
    pub archetype_id: String,
    pub deviation_key: String,
    pub observation_count: usize,
    pub repo_ids: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTierCalibrationTarget {
    Planner,
    Ladder,
}

/// One independently archived signal supporting a planner-rubric or ladder-cell change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskTierCalibrationObservation {
    pub repo_id: String,
    pub session_id: String,
    pub archive_id: String,
    pub target: TaskTierCalibrationTarget,
    pub task_tier: TaskTier,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_tier: Option<TaskTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_flags: Vec<String>,
    #[serde(default)]
    pub node_ids: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

/// Review-gated calibration proposal keyed explicitly by task tier and provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskTierCalibrationProposal {
    pub promotion_tier: PromotionTier,
    pub target: TaskTierCalibrationTarget,
    pub task_tier: TaskTier,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_tier: Option<TaskTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_flags: Vec<String>,
    pub observation_count: usize,
    pub repo_ids: Vec<String>,
    pub session_ids: Vec<String>,
    pub archive_ids: Vec<String>,
    #[serde(default)]
    pub node_ids: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    pub rationale: String,
}

/// Return in-memory calibration proposals only. This function has no filesystem,
/// ladder, or planner write handle and therefore cannot apply a proposed change.
pub fn propose_task_tier_calibrations(
    observations: &[TaskTierCalibrationObservation],
) -> Vec<TaskTierCalibrationProposal> {
    type CalibrationKey = (
        TaskTierCalibrationTarget,
        TaskTier,
        String,
        Option<TaskTier>,
        Option<String>,
        Vec<String>,
    );
    let mut groups: BTreeMap<
        CalibrationKey,
        BTreeMap<(String, String, String), &TaskTierCalibrationObservation>,
    > = BTreeMap::new();
    for observation in observations {
        groups
            .entry((
                observation.target,
                observation.task_tier,
                observation.provider.clone(),
                observation.candidate_tier,
                observation.candidate_model.clone(),
                observation.candidate_flags.clone(),
            ))
            .or_default()
            .entry((
                observation.repo_id.clone(),
                observation.session_id.clone(),
                observation.archive_id.clone(),
            ))
            .or_insert(observation);
    }

    let mut proposals = Vec::new();
    for (
        (target, task_tier, provider, candidate_tier, candidate_model, candidate_flags),
        instances,
    ) in groups
    {
        let repo_ids: BTreeSet<_> = instances
            .values()
            .map(|item| item.repo_id.clone())
            .collect();
        if repo_ids.len() < 2 && instances.len() < 2 {
            continue;
        }
        let promotion_tier = if repo_ids.len() >= 2 {
            PromotionTier::InstitutionalRevision
        } else {
            PromotionTier::ProjectOverride
        };
        let session_ids: BTreeSet<_> = instances
            .values()
            .map(|item| item.session_id.clone())
            .collect();
        let archive_ids: BTreeSet<_> = instances
            .values()
            .map(|item| item.archive_id.clone())
            .collect();
        let node_ids: BTreeSet<_> = instances
            .values()
            .flat_map(|item| item.node_ids.iter().cloned())
            .collect();
        let evidence_refs: BTreeSet<_> = instances
            .values()
            .flat_map(|item| item.evidence_refs.iter().cloned())
            .collect();
        proposals.push(TaskTierCalibrationProposal {
            promotion_tier,
            target,
            task_tier,
            provider,
            candidate_tier,
            candidate_model,
            candidate_flags,
            observation_count: instances.len(),
            repo_ids: repo_ids.into_iter().collect(),
            session_ids: session_ids.into_iter().collect(),
            archive_ids: archive_ids.into_iter().collect(),
            node_ids: node_ids.into_iter().collect(),
            evidence_refs: evidence_refs.into_iter().collect(),
            rationale: match target {
                TaskTierCalibrationTarget::Planner => "remediation affected more than half of the observed low- or medium-tier runs; propose raising the planner rubric or archetype default for human review".to_string(),
                TaskTierCalibrationTarget::Ladder => "a repeated explicit execution override matched the configured cell's observed escape rate; propose reviewing that override as a replacement ladder cell".to_string(),
            },
        });
    }
    proposals
}

/// Return in-memory proposals only. This function has no filesystem argument
/// and cannot write a project override or institutional wiki revision.
pub fn propose_deviation_promotions(
    observations: &[DeviationObservation],
) -> Vec<DeviationPromotionProposal> {
    let mut groups: BTreeMap<(String, String), Vec<&DeviationObservation>> = BTreeMap::new();
    for observation in observations {
        groups
            .entry((
                observation.archetype_id.clone(),
                observation.deviation_key.clone(),
            ))
            .or_default()
            .push(observation);
    }

    let mut proposals = Vec::new();
    for ((archetype_id, deviation_key), group) in groups {
        let mut by_repo: BTreeMap<String, usize> = BTreeMap::new();
        for observation in &group {
            *by_repo.entry(observation.repo_id.clone()).or_default() += 1;
        }
        let repo_ids: Vec<_> = by_repo.keys().cloned().collect();
        if repo_ids.len() >= 2 {
            proposals.push(DeviationPromotionProposal {
                tier: PromotionTier::InstitutionalRevision,
                archetype_id,
                deviation_key,
                observation_count: group.len(),
                repo_ids,
                rationale: "the same deviation recurred across at least two repositories; propose a PR-gated Tier 2 archetype revision".to_string(),
            });
            continue;
        }
        for (repo_id, count) in by_repo.into_iter().filter(|(_, count)| *count >= 2) {
            proposals.push(DeviationPromotionProposal {
                tier: PromotionTier::ProjectOverride,
                archetype_id: archetype_id.clone(),
                deviation_key: deviation_key.clone(),
                observation_count: count,
                repo_ids: vec![repo_id],
                rationale: "the same deviation recurred in one repository; propose a Tier 1 standing override".to_string(),
            });
        }
    }
    proposals
}

pub fn built_in_archetypes() -> Vec<GraphArchetype> {
    vec![
        feature_build_archetype(),
        bug_hunt_archetype(),
        migration_archetype(),
        audit_archetype(),
    ]
}

fn feature_build_archetype() -> GraphArchetype {
    GraphArchetype {
        id: "feature-build".to_string(),
        version: 1,
        description: "plan, implement, integrate, and review a feature".to_string(),
        holes: vec!["component".to_string()],
        parameters: BTreeMap::from([("component".to_string(), "feature".to_string())]),
        lanes: vec![
            lane("design", "Design ${component}", "plan", &[]),
            lane("backend", "Build ${component} backend", "code", &["design"]).requiring("backend"),
            lane(
                "frontend",
                "Build ${component} frontend",
                "code",
                &["design"],
            )
            .requiring("frontend"),
            lane(
                "integrate",
                "Integrate ${component}",
                "code",
                &["backend", "frontend"],
            ),
        ],
        review_templates: vec![ReviewTemplate::code_tasks("code-review")],
        checkpoints: Vec::new(),
    }
}

fn bug_hunt_archetype() -> GraphArchetype {
    GraphArchetype {
        id: "bug-hunt".to_string(),
        version: 1,
        description: "reproduce, isolate, fix, and regression-review a defect".to_string(),
        holes: vec!["component".to_string()],
        parameters: BTreeMap::from([("component".to_string(), "defect".to_string())]),
        lanes: vec![
            lane("reproduce", "Reproduce ${component}", "evidence", &[]),
            lane(
                "isolate",
                "Isolate ${component}",
                "root-cause",
                &["reproduce"],
            ),
            lane("fix", "Fix ${component}", "code", &["isolate"]),
            lane("verify", "Verify ${component}", "evidence", &["fix"]),
        ],
        review_templates: vec![ReviewTemplate::code_tasks("fix-review")],
        checkpoints: Vec::new(),
    }
}

fn migration_archetype() -> GraphArchetype {
    let checkpoint = CheckpointWave::new(
        "migration-gate",
        vec!["migrate".to_string(), "verify".to_string()],
        vec!["rollout".to_string()],
    )
    .enabled();
    GraphArchetype {
        id: "migration".to_string(),
        version: 1,
        description: "prepare, apply, verify, gate, and roll out a migration".to_string(),
        holes: vec!["component".to_string()],
        parameters: BTreeMap::from([("component".to_string(), "migration".to_string())]),
        lanes: vec![
            lane("inventory", "Inventory ${component}", "plan", &[]),
            lane("migrate", "Apply ${component}", "code", &["inventory"]),
            lane("verify", "Verify ${component}", "evidence", &["migrate"]),
            lane("rollout", "Roll out ${component}", "artifact", &["verify"]),
        ],
        review_templates: vec![ReviewTemplate::code_tasks("migration-review")],
        checkpoints: vec![checkpoint],
    }
}

fn audit_archetype() -> GraphArchetype {
    let mut review = ReviewTemplate::code_tasks("finding-review");
    review.required_output = Some("finding".to_string());
    GraphArchetype {
        id: "audit".to_string(),
        version: 1,
        description: "scope, inspect, reconcile, and report an audit".to_string(),
        holes: vec!["component".to_string()],
        parameters: BTreeMap::from([("component".to_string(), "system".to_string())]),
        lanes: vec![
            lane("scope", "Scope ${component} audit", "plan", &[]),
            lane("inspect", "Inspect ${component}", "finding", &["scope"]),
            lane(
                "reconcile",
                "Reconcile ${component} findings",
                "finding",
                &["inspect"],
            ),
            lane(
                "report",
                "Report ${component} audit",
                "artifact",
                &["reconcile"],
            ),
        ],
        review_templates: vec![review],
        checkpoints: Vec::new(),
    }
}

fn lane(id: &str, title: &str, output: &str, depends_on: &[&str]) -> ArchetypeLane {
    ArchetypeLane {
        id: id.to_string(),
        title: title.to_string(),
        kind: NodeKind::Task,
        contract: NodeContract {
            inputs: Vec::new(),
            outputs: vec![output.to_string()],
            acceptance: vec![format!("{title} satisfies its declared contract")],
        },
        binding: BindingRef::Role("backend".to_string()),
        depends_on: depends_on.iter().map(|id| (*id).to_string()).collect(),
        required_repo_facts: Vec::new(),
    }
}

impl ArchetypeLane {
    fn requiring(mut self, fact: &str) -> Self {
        self.required_repo_facts.push(fact.to_string());
        self
    }
}
