//! Deterministic `/codegraph` artifact integration for issue #215.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::WorkspaceStrategy;

use super::archetypes::{RepoShapeFacts, RepoShapeFactsProvider};
use super::context::TouchesResolver;
use super::review::checkpoint_aware_claimable_nodes;
use super::{
    BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract, NodeKind,
    NodeStatus, TaskGraph, TaskId, WorkEdge, WorkGraphOmission, WorkGraphOmissionReason,
    WorkNode,
};

pub const CODEGRAPH_MODULE_TEMPLATE: &str = "codegraph-module";
pub const MAX_ARTIFACT_MODULES: usize = 50_000;
pub const MAX_TOUCH_MODULES_PER_TASK: usize = 256;
pub const MAX_MODULE_PATH_CHARS: usize = 512;
const MAX_OMISSION_EXAMPLES: usize = 5;

#[derive(Debug, Deserialize)]
struct RawCodegraphArtifact {
    root: String,
    language: String,
    #[serde(default)]
    nodes: BTreeMap<String, RawCodegraphNode>,
}

#[derive(Debug, Deserialize)]
struct RawCodegraphNode {
    path: String,
}

/// A validated view of the JSON emitted by
/// `codegraph.py build --root <ROOT> --out graph.json`.
///
/// The artifact is externally generated and read-only. Filtering is repeated
/// here because an artifact may be stale or come from an older analyzer that
/// indexed agent worktrees or generated trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactCodegraph {
    project_root: PathBuf,
    language: Option<String>,
    modules: BTreeSet<String>,
    module_aliases: BTreeMap<String, BTreeSet<String>>,
    available: bool,
}

impl ArtifactCodegraph {
    pub fn load(project_path: &Path, artifact_path: &Path) -> Result<Self, String> {
        let project_root = canonical_project_root(project_path)?;
        match fs::read_to_string(artifact_path) {
            Ok(json) => Self::from_json_with_root(project_root, &json),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                project_root,
                language: None,
                modules: BTreeSet::new(),
                module_aliases: BTreeMap::new(),
                available: false,
            }),
            Err(error) => Err(format!(
                "cannot read codegraph artifact {}: {error}",
                artifact_path.display()
            )),
        }
    }

    pub fn from_json(project_path: &Path, json: &str) -> Result<Self, String> {
        Self::from_json_with_root(canonical_project_root(project_path)?, json)
    }

    fn from_json_with_root(project_root: PathBuf, json: &str) -> Result<Self, String> {
        let raw: RawCodegraphArtifact = serde_json::from_str(json)
            .map_err(|error| format!("invalid codegraph JSON: {error}"))?;
        let artifact_root = fs::canonicalize(Path::new(&raw.root)).map_err(|error| {
            format!("cannot resolve codegraph artifact root {}: {error}", raw.root)
        })?;
        if artifact_root != project_root {
            return Err(format!(
                "codegraph artifact root {} does not match project root {}",
                artifact_root.display(),
                project_root.display()
            ));
        }
        if raw.nodes.len() > MAX_ARTIFACT_MODULES {
            return Err(format!(
                "codegraph artifact module cap {MAX_ARTIFACT_MODULES} exceeded"
            ));
        }

        let mut modules = BTreeSet::new();
        let mut module_aliases: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (module_id, raw_node) in raw.nodes {
            let Some(path) = normalize_artifact_path(&raw_node.path) else {
                continue;
            };
            if path_is_excluded(&path) || path.chars().count() > MAX_MODULE_PATH_CHARS {
                continue;
            }
            modules.insert(path.clone());
            for alias in module_alias_candidates(&module_id, &path) {
                module_aliases.entry(alias).or_default().insert(path.clone());
            }
        }

        Ok(Self {
            project_root,
            language: Some(raw.language.to_ascii_lowercase()),
            modules,
            module_aliases,
            available: true,
        })
    }

    pub fn artifact_language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    pub fn indexed_modules(&self) -> &BTreeSet<String> {
        &self.modules
    }

    pub fn is_available(&self) -> bool {
        self.available
    }

    fn resolve_graph(&self, graph: &TaskGraph) -> BTreeMap<TaskId, BTreeSet<String>> {
        let mut resolved = BTreeMap::new();
        for node in graph.nodes.iter().filter(|node| node.kind == NodeKind::Task) {
            let intents = explicit_touch_intents(node);
            if intents.is_empty() {
                continue;
            }

            let mut modules = BTreeSet::new();
            let mut complete = true;
            for intent in intents {
                match intent {
                    TouchIntent::None => {}
                    TouchIntent::Path(value) => {
                        match resolve_intent(&self.modules, &self.module_aliases, &value) {
                            Ok(matches) => modules.extend(matches),
                            Err(()) => complete = false,
                        }
                    }
                }
            }
            if complete && modules.len() <= MAX_TOUCH_MODULES_PER_TASK {
                resolved.insert(node.id.clone(), modules);
            }
        }
        resolved
    }
}

impl TouchesResolver for ArtifactCodegraph {
    fn resolve_touches(
        &self,
        graph: &TaskGraph,
    ) -> Result<Option<BTreeMap<TaskId, BTreeSet<String>>>, String> {
        if !self.available {
            return Ok(None);
        }
        Ok(Some(self.resolve_graph(graph)))
    }
}

impl RepoShapeFactsProvider for ArtifactCodegraph {
    fn facts(&self, project_path: &Path) -> Result<Option<RepoShapeFacts>, String> {
        if !self.available {
            return Ok(None);
        }
        let requested_root = canonical_project_root(project_path)?;
        if requested_root != self.project_root {
            return Err(format!(
                "repo-shape root {} does not match artifact root {}",
                requested_root.display(),
                self.project_root.display()
            ));
        }
        Ok(Some(repo_shape_facts(
            self.language.as_deref(),
            &self.modules,
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphDerivationReport {
    pub available: bool,
    pub touches: BTreeMap<TaskId, BTreeSet<String>>,
    pub unresolved_task_ids: Vec<TaskId>,
    pub module_node_count: usize,
    pub touch_edge_count: usize,
}

/// Rebuild module nodes and Codegraph-provenance `Touches` edges. Missing
/// artifact, unreadable source, undeclared intent, and uncovered language are
/// distinct omission states; none is allowed to read as a clean zero.
pub fn derive_codegraph_touches<R: TouchesResolver>(
    graph: &mut TaskGraph,
    resolver: &R,
) -> CodegraphDerivationReport {
    clear_derived_codegraph(graph);
    let touches = match resolver.resolve_touches(graph) {
        Ok(Some(touches)) => touches,
        Ok(None) => {
            graph.omissions.push(WorkGraphOmission::new(
                WorkGraphOmissionReason::CodegraphUnavailable,
                1,
                vec!["touches-resolver".to_string()],
            ));
            return CodegraphDerivationReport {
                available: false,
                touches: BTreeMap::new(),
                unresolved_task_ids: Vec::new(),
                module_node_count: 0,
                touch_edge_count: 0,
            };
        }
        Err(error) => {
            graph.omissions.push(WorkGraphOmission::new(
                WorkGraphOmissionReason::SourceUnreadable,
                1,
                vec![format!("touches-resolver: {error}")],
            ));
            return CodegraphDerivationReport {
                available: false,
                touches: BTreeMap::new(),
                unresolved_task_ids: Vec::new(),
                module_node_count: 0,
                touch_edge_count: 0,
            };
        }
    };

    let tasks: BTreeMap<_, _> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Task)
        .map(|node| (node.id.clone(), node.clone()))
        .collect();
    let unresolved_task_ids: Vec<_> = tasks
        .keys()
        .filter(|task_id| !touches.contains_key(*task_id))
        .cloned()
        .collect();
    add_resolution_omissions(graph, &tasks, &unresolved_task_ids);

    let unknown: Vec<_> = touches
        .keys()
        .filter(|task_id| !tasks.contains_key(*task_id))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        graph.omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::ResolutionIncomplete,
            unknown.len(),
            unknown.into_iter().take(MAX_OMISSION_EXAMPLES).collect(),
        ));
    }

    let modules: BTreeSet<_> = touches
        .iter()
        .filter(|(task_id, _)| tasks.contains_key(*task_id))
        .flat_map(|(_, modules)| modules.iter().cloned())
        .collect();
    for module in &modules {
        graph.nodes.push(module_node(module));
    }

    let mut touch_edge_count = 0;
    for (task_id, task_modules) in &touches {
        if !tasks.contains_key(task_id) {
            continue;
        }
        for module in task_modules {
            graph.edges.push(
                WorkEdge::new(
                    task_id,
                    module_node_id(module),
                    EdgeKind::Touches,
                    EdgeProvenance::Codegraph,
                )
                .with_rationale(format!("explicit task intent resolved to module {module}")),
            );
            touch_edge_count += 1;
        }
    }

    CodegraphDerivationReport {
        available: true,
        touches,
        unresolved_task_ids,
        module_node_count: modules.len(),
        touch_edge_count,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictDetectionState {
    Disabled,
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelConflictAction {
    Serialize,
    WorktreeIsolate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyTaskConflict {
    pub first_task_id: TaskId,
    pub second_task_id: TaskId,
    pub overlapping_modules: Vec<String>,
    pub action: ParallelConflictAction,
    /// Stable payload for the claim path to persist/log when it wires this
    /// pure decision helper into the atomic queue boundary.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictDetectionReport {
    pub state: ConflictDetectionState,
    pub decisions: Vec<ReadyTaskConflict>,
    pub unresolved_ready_task_ids: Vec<TaskId>,
}

/// Pure claim-boundary conflict policy. `None` disables detection explicitly;
/// `Some(empty)` is a complete clean result. Only checkpoint-aware claimable
/// tasks participate. The returned reason is the logging/persistence payload;
/// queue enforcement is intentionally outside this owned module.
pub fn conflicting_ready_tasks(
    graph: &TaskGraph,
    resolved_touches: Option<&BTreeMap<TaskId, BTreeSet<String>>>,
    workspace_strategy: WorkspaceStrategy,
) -> ConflictDetectionReport {
    let Some(resolved_touches) = resolved_touches else {
        return ConflictDetectionReport {
            state: ConflictDetectionState::Disabled,
            decisions: Vec::new(),
            unresolved_ready_task_ids: Vec::new(),
        };
    };
    let ready = checkpoint_aware_claimable_nodes(graph);
    if resolved_touches.is_empty() {
        return ConflictDetectionReport {
            state: ConflictDetectionState::Complete,
            decisions: Vec::new(),
            unresolved_ready_task_ids: Vec::new(),
        };
    }

    let unresolved_ready_task_ids: Vec<_> = ready
        .iter()
        .filter(|task_id| !resolved_touches.contains_key(*task_id))
        .cloned()
        .collect();
    let state = if unresolved_ready_task_ids.is_empty() {
        ConflictDetectionState::Complete
    } else {
        ConflictDetectionState::Partial
    };
    let mut decisions = Vec::new();
    for left_index in 0..ready.len() {
        for right_index in (left_index + 1)..ready.len() {
            let first = &ready[left_index];
            let second = &ready[right_index];
            let (Some(first_touches), Some(second_touches)) =
                (resolved_touches.get(first), resolved_touches.get(second))
            else {
                continue;
            };
            let overlapping_modules: Vec<_> = first_touches
                .intersection(second_touches)
                .cloned()
                .collect();
            if overlapping_modules.is_empty() {
                continue;
            }
            let (action, verb) = match workspace_strategy {
                WorkspaceStrategy::IsolatedCell => {
                    (ParallelConflictAction::WorktreeIsolate, "isolate claims")
                }
                WorkspaceStrategy::SharedCell | WorkspaceStrategy::None => {
                    (ParallelConflictAction::Serialize, "serialize claims")
                }
            };
            decisions.push(ReadyTaskConflict {
                first_task_id: first.clone(),
                second_task_id: second.clone(),
                reason: format!(
                    "ready tasks {first} and {second} overlap codegraph modules [{}]; {verb} because workspace strategy is {}",
                    overlapping_modules.join(", "),
                    workspace_strategy_label(workspace_strategy),
                ),
                overlapping_modules,
                action,
            });
        }
    }
    ConflictDetectionReport {
        state,
        decisions,
        unresolved_ready_task_ids,
    }
}

fn canonical_project_root(project_path: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(project_path).map_err(|error| {
        format!("cannot resolve codegraph root {}: {error}", project_path.display())
    })?;
    if !root.is_dir() {
        return Err(format!("codegraph root is not a directory: {}", root.display()));
    }
    Ok(root)
}

fn normalize_artifact_path(value: &str) -> Option<String> {
    let normalized = value.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return None;
    }
    let components: Vec<_> = normalized
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect();
    if components.is_empty() || components.iter().any(|component| *component == "..") {
        return None;
    }
    Some(components.join("/").to_ascii_lowercase())
}

fn path_is_excluded(path: &str) -> bool {
    path.split('/').any(|component| {
        matches!(
            component,
            ".hive-manager"
                | ".hive-fusion"
                | ".hive-debate"
                | ".git"
                | ".hg"
                | ".svn"
                | ".claude"
                | ".codex"
                | ".agents"
                | ".svelte-kit"
                | ".next"
                | ".turbo"
                | ".cache"
                | ".vite"
                | "node_modules"
                | "target"
                | "dist"
                | "build"
                | "coverage"
                | "package"
                | "out"
        )
    })
}

fn module_alias_candidates(module_id: &str, path: &str) -> BTreeSet<String> {
    let mut aliases = BTreeSet::from([path.to_string()]);
    if let Some((extensionless, _)) = path.rsplit_once('.') {
        aliases.insert(extensionless.to_string());
    }
    let normalized_id = module_id.replace('\\', "/").to_ascii_lowercase();
    if !Path::new(&normalized_id).is_absolute() {
        aliases.insert(normalized_id);
    }
    aliases
}

fn resolve_intent(
    modules: &BTreeSet<String>,
    aliases: &BTreeMap<String, BTreeSet<String>>,
    intent: &str,
) -> Result<BTreeSet<String>, ()> {
    if modules.contains(intent) {
        return Ok(BTreeSet::from([intent.to_string()]));
    }
    if let Some(matches) = aliases.get(intent) {
        if matches.len() == 1 {
            return Ok(matches.clone());
        }
        return Err(());
    }
    let prefix = format!("{intent}/");
    let descendants: BTreeSet<_> = modules
        .iter()
        .filter(|module| module.starts_with(&prefix))
        .take(MAX_TOUCH_MODULES_PER_TASK + 1)
        .cloned()
        .collect();
    if descendants.is_empty() || descendants.len() > MAX_TOUCH_MODULES_PER_TASK {
        Err(())
    } else {
        Ok(descendants)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TouchIntent {
    None,
    Path(String),
}

fn explicit_touch_intents(node: &WorkNode) -> Vec<TouchIntent> {
    node.contract
        .inputs
        .iter()
        .chain(node.contract.outputs.iter())
        .chain(node.contract.acceptance.iter())
        .filter_map(|value| {
            let trimmed = value.trim();
            let lower = trimmed.to_ascii_lowercase();
            let raw = ["touch:", "file:", "module:"]
                .iter()
                .find_map(|prefix| lower.starts_with(prefix).then(|| &trimmed[prefix.len()..]))?;
            let normalized = raw
                .trim()
                .trim_matches(|character| matches!(character, '`' | '"' | '\'' | ',' | ';'));
            if normalized.eq_ignore_ascii_case("none") {
                return Some(TouchIntent::None);
            }
            normalize_artifact_path(normalized).map(TouchIntent::Path)
        })
        .collect()
}

fn add_resolution_omissions(
    graph: &mut TaskGraph,
    tasks: &BTreeMap<TaskId, WorkNode>,
    unresolved_task_ids: &[TaskId],
) {
    let mut undeclared = Vec::new();
    let mut uncovered = Vec::new();
    let mut uncovered_languages = BTreeSet::new();
    for task_id in unresolved_task_ids {
        let node = tasks[task_id];
        let intents = explicit_touch_intents(&node);
        if intents.is_empty() {
            undeclared.push(task_id.clone());
            continue;
        }
        for intent in intents {
            if let TouchIntent::Path(path) = intent {
                if let Some(language) = source_language(&path) {
                    uncovered_languages.insert(language.to_string());
                }
            }
        }
        uncovered.push(task_id.clone());
    }
    if !undeclared.is_empty() {
        let mut omission = WorkGraphOmission::new(
            WorkGraphOmissionReason::ResolutionIncomplete,
            undeclared.len(),
            undeclared
                .into_iter()
                .take(MAX_OMISSION_EXAMPLES)
                .collect(),
        );
        omission.detail = "codegraph artifact was available, but explicit task touch intent was not declared".to_string();
        graph.omissions.push(omission);
    }
    if !uncovered.is_empty() {
        let mut omission = WorkGraphOmission::new(
            WorkGraphOmissionReason::ResolutionIncomplete,
            uncovered.len(),
            uncovered
                .into_iter()
                .take(MAX_OMISSION_EXAMPLES)
                .collect(),
        );
        let languages = if uncovered_languages.is_empty() {
            "unknown".to_string()
        } else {
            uncovered_languages.into_iter().collect::<Vec<_>>().join(", ")
        };
        omission.detail = format!(
            "codegraph artifact was available but did not cover or resolve declared language(s): {languages}"
        );
        graph.omissions.push(omission);
    }
}

fn source_language(path: &str) -> Option<&'static str> {
    match path.rsplit_once('.')?.1 {
        "rs" => Some("rust"),
        "py" | "pyi" => Some("python"),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some("typescript/javascript"),
        "go" => Some("go"),
        "java" | "kt" | "kts" => Some("jvm"),
        "cs" => Some("csharp"),
        "c" | "cc" | "cpp" | "h" | "hpp" => Some("c/cpp"),
        other if !other.is_empty() => Some("other"),
        _ => None,
    }
}

fn repo_shape_facts(language: Option<&str>, modules: &BTreeSet<String>) -> RepoShapeFacts {
    let mut facts = BTreeSet::new();
    if let Some(language) = language {
        facts.insert(format!("codegraph:{language}"));
    }
    for module in modules {
        if let Some(language) = source_language(module) {
            facts.insert(format!("language:{language}"));
        }
        if source_language(module) == Some("python")
            || has_path_component(module, &["backend", "server", "api"])
        {
            facts.insert("backend".to_string());
        }
        if source_language(module) == Some("typescript/javascript")
            || has_path_component(module, &["frontend", "client", "web", "routes"])
        {
            facts.insert("frontend".to_string());
        }
        if has_path_component(module, &["test", "tests", "spec", "specs"])
            || module.contains(".test.")
            || module.contains(".spec.")
        {
            facts.insert("tests".to_string());
        }
    }
    RepoShapeFacts { facts }
}

fn has_path_component(module: &str, expected: &[&str]) -> bool {
    module
        .split('/')
        .any(|component| expected.contains(&component))
}

fn module_node(module: &str) -> WorkNode {
    let mut parameters = BTreeMap::new();
    parameters.insert("module".to_string(), module.to_string());
    let mut node = WorkNode::new(
        module_node_id(module),
        NodeKind::Context,
        format!("Module {module}"),
        NodeContract {
            inputs: Vec::new(),
            outputs: vec![format!("module:{module}")],
            acceptance: Vec::new(),
        },
        BindingRef::Zone("codegraph".to_string()),
        NodeStatus::Completed,
    );
    node.expansion = Some(CompositeExpansion {
        template: CODEGRAPH_MODULE_TEMPLATE.to_string(),
        parameters,
    });
    node
}

fn module_node_id(module: &str) -> TaskId {
    format!(
        "codegraph::module::{}",
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("hive-manager:codegraph:{module}").as_bytes()
        )
    )
}

fn clear_derived_codegraph(graph: &mut TaskGraph) {
    let module_ids: BTreeSet<_> = graph
        .nodes
        .iter()
        .filter(|node| {
            node.expansion
                .as_ref()
                .is_some_and(|expansion| expansion.template == CODEGRAPH_MODULE_TEMPLATE)
        })
        .map(|node| node.id.clone())
        .collect();
    graph.nodes.retain(|node| !module_ids.contains(&node.id));
    graph.edges.retain(|edge| {
        !(edge.kind == EdgeKind::Touches && edge.provenance == EdgeProvenance::Codegraph)
            && !module_ids.contains(&edge.source)
            && !module_ids.contains(&edge.target)
    });
}

fn workspace_strategy_label(strategy: WorkspaceStrategy) -> &'static str {
    match strategy {
        WorkspaceStrategy::SharedCell => "shared_cell",
        WorkspaceStrategy::IsolatedCell => "isolated_cell",
        WorkspaceStrategy::None => "none",
    }
}
