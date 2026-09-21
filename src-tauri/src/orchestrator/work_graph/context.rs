//! Strictly-derived project-knowledge context for issue #218.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::actions::git::run_git_in_dir;
use crate::http::handlers::knowledge::{first_h1, frontmatter_field, split_frontmatter};

use super::archetypes::{GotchaAttachment, GotchaAttachmentProvider};
use super::{
    BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract, NodeKind,
    NodeStatus, TaskGraph, TaskId, WorkEdge, WorkGraphOmission, WorkGraphOmissionReason,
    WorkNode,
};

pub const MAX_CONTEXT_SUMMARY_CHARS: usize = 240;
pub const MAX_DERIVED_CONTEXT_NODES: usize = 128;
pub const MAX_CONTEXT_SCOPES_PER_GOTCHA: usize = 16;
pub const MAX_CONTEXT_SCOPE_CHARS: usize = 256;
pub const DERIVED_CONTEXT_TEMPLATE: &str = "derived-project-context";
/// Context linked to 75% or more of a multi-task plan is usually standing
/// guidance, not discriminating task context. It is flagged and withheld from
/// task edges so one generic gotcha cannot become a prompt-dominating hub.
pub const ANTI_HUB_TASK_FRACTION: f64 = 0.75;
pub const ANTI_HUB_MIN_TASKS: usize = 2;
pub const ENABLE_KNOWLEDGE_PER_INTENT: bool = false;
pub const ENABLE_KNOWLEDGE_CONTRACT_HARVEST: bool = false;
pub const ENABLE_KNOWLEDGE_EXACT_MATCH: bool = false;
pub const ENABLE_KNOWLEDGE_UNIQUE_BASENAME_MATCH: bool = false;
pub const ENABLE_KNOWLEDGE_PATH_SUFFIX_MATCH: bool = false;
pub const ENABLE_KNOWLEDGE_PARENT_DIRECTORY_MATCH: bool = false;
pub const ENABLE_KNOWLEDGE_AMBIGUOUS_MATCH: bool = false;
pub const ENABLE_KNOWLEDGE_RESOLUTION_OMISSIONS: bool = false;
pub const ENABLE_KNOWLEDGE_INFERRED_SCOPE: bool = false;
pub const ENABLE_KNOWLEDGE_FILE_INVENTORY_FALLBACK: bool = false;
const MAX_OMISSION_EXAMPLES: usize = 5;
const MAX_KNOWLEDGE_TOUCHES_PER_TASK: usize = 256;
const MAX_FILE_INVENTORY_ENTRIES: usize = 50_000;
const MAX_FILE_INVENTORY_PATH_CHARS: usize = 512;

/// Lossless codegraph/touches coverage shared by enrichment, context, and
/// claim-time conflict projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TouchCoverageReport {
    pub available: bool,
    #[serde(default)]
    pub artifact_languages: BTreeSet<String>,
    #[serde(default)]
    pub touches: BTreeMap<TaskId, BTreeSet<String>>,
    #[serde(default)]
    pub unresolved_task_ids: Vec<TaskId>,
}

impl TouchCoverageReport {
    pub fn unavailable() -> Self {
        Self {
            available: false,
            artifact_languages: BTreeSet::new(),
            touches: BTreeMap::new(),
            unresolved_task_ids: Vec::new(),
        }
    }
}

impl Default for TouchCoverageReport {
    fn default() -> Self {
        Self::unavailable()
    }
}

/// Codegraph integration seam implemented by WS-8 (#215).
pub trait TouchesResolver {
    fn resolve_touches(
        &self,
        graph: &TaskGraph,
    ) -> Result<TouchCoverageReport, String>;

    fn knowledge_candidates(&self) -> Option<BTreeSet<String>> {
        None
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoTouchesResolver;

impl TouchesResolver for NoTouchesResolver {
    fn resolve_touches(
        &self,
        _graph: &TaskGraph,
    ) -> Result<TouchCoverageReport, String> {
        Ok(TouchCoverageReport::unavailable())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathMatchType {
    Exact,
    UniqueBasename,
    PathSuffix,
    ParentDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathResolution {
    pub path: String,
    pub match_type: PathMatchType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathResolutionFailure {
    Ambiguous,
    NoMatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KnowledgeAttachmentConfig {
    pub per_intent: bool,
    pub contract_harvest: bool,
    pub exact: bool,
    pub unique_basename: bool,
    pub path_suffix: bool,
    pub parent_directory: bool,
    pub ambiguous: bool,
    pub resolution_omissions: bool,
    pub inferred_scope: bool,
    pub file_inventory_fallback: bool,
}

impl KnowledgeAttachmentConfig {
    pub(crate) const fn production() -> Self {
        Self {
            per_intent: ENABLE_KNOWLEDGE_PER_INTENT,
            contract_harvest: ENABLE_KNOWLEDGE_CONTRACT_HARVEST,
            exact: ENABLE_KNOWLEDGE_EXACT_MATCH,
            unique_basename: ENABLE_KNOWLEDGE_UNIQUE_BASENAME_MATCH,
            path_suffix: ENABLE_KNOWLEDGE_PATH_SUFFIX_MATCH,
            parent_directory: ENABLE_KNOWLEDGE_PARENT_DIRECTORY_MATCH,
            ambiguous: ENABLE_KNOWLEDGE_AMBIGUOUS_MATCH,
            resolution_omissions: ENABLE_KNOWLEDGE_RESOLUTION_OMISSIONS,
            inferred_scope: ENABLE_KNOWLEDGE_INFERRED_SCOPE,
            file_inventory_fallback: ENABLE_KNOWLEDGE_FILE_INVENTORY_FALLBACK,
        }
    }

    fn enables(self, match_type: PathMatchType) -> bool {
        match match_type {
            PathMatchType::Exact => self.exact,
            PathMatchType::UniqueBasename => self.unique_basename,
            PathMatchType::PathSuffix => self.path_suffix,
            PathMatchType::ParentDirectory => self.parent_directory,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnowledgeTouchCoverage {
    pub declared_touches: BTreeMap<TaskId, BTreeSet<String>>,
    pub knowledge_attachment_touches: BTreeMap<TaskId, BTreeSet<String>>,
    pub provenance_by_task: BTreeMap<TaskId, BTreeMap<String, BTreeSet<String>>>,
    pub resolution_omissions: Vec<WorkGraphOmission>,
}

pub(crate) fn build_knowledge_touch_coverage<R: TouchesResolver>(
    graph: &TaskGraph,
    resolver: &R,
    declared_coverage: &TouchCoverageReport,
    file_inventory: Option<&BTreeSet<String>>,
    config: KnowledgeAttachmentConfig,
) -> KnowledgeTouchCoverage {
    let declared_touches = declared_coverage.touches.clone();
    let mut knowledge_attachment_touches = declared_touches.clone();
    let mut provenance_by_task = BTreeMap::new();
    for (task_id, paths) in &declared_touches {
        for path in paths {
            record_knowledge_provenance(
                &mut provenance_by_task,
                task_id,
                path,
                "declared-scope",
            );
        }
    }

    let artifact_candidates = resolver.knowledge_candidates();
    let fallback_inventory = config.file_inventory_fallback.then_some(file_inventory).flatten();
    let candidates = artifact_candidates.as_ref().or(fallback_inventory);
    let fallback = artifact_candidates.is_none() && fallback_inventory.is_some();
    let mut resolution_omissions = Vec::new();
    if let Some(candidates) = candidates {
        for node in graph.nodes.iter().filter(|node| node.kind == NodeKind::Task) {
            if config.per_intent {
                for intent in declared_contract_path_intents(node) {
                    attach_knowledge_intent(
                        &node.id,
                        &intent,
                        "declared-scope",
                        candidates,
                        fallback,
                        config,
                        &mut knowledge_attachment_touches,
                        &mut provenance_by_task,
                        &mut resolution_omissions,
                    );
                }
            }
            if config.contract_harvest {
                for intent in harvest_contract_path_intents(node) {
                    attach_knowledge_intent(
                        &node.id,
                        &intent,
                        "contract-path",
                        candidates,
                        fallback,
                        config,
                        &mut knowledge_attachment_touches,
                        &mut provenance_by_task,
                        &mut resolution_omissions,
                    );
                }
            }
        }
    }

    KnowledgeTouchCoverage {
        declared_touches,
        knowledge_attachment_touches,
        provenance_by_task,
        resolution_omissions: aggregate_resolution_omissions(resolution_omissions),
    }
}

fn aggregate_resolution_omissions(
    omissions: Vec<WorkGraphOmission>,
) -> Vec<WorkGraphOmission> {
    let mut grouped: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    for omission in omissions {
        let entry = grouped.entry(omission.detail).or_default();
        entry.0 += omission.count;
        entry.1.extend(omission.examples);
    }
    grouped
        .into_iter()
        .map(|(detail, (count, examples))| {
            let mut omission = WorkGraphOmission::new(
                WorkGraphOmissionReason::ResolutionIncomplete,
                count,
                examples.into_iter().take(MAX_OMISSION_EXAMPLES).collect(),
            );
            omission.detail = detail;
            omission
        })
        .collect()
}

/// Load the repository's tracked-file inventory without exposing Git or OS
/// diagnostics in graph output. Callers turn the stable error category into a
/// fail-open omission.
pub(crate) fn load_tracked_file_inventory(
    inventory_root: &Path,
) -> Result<BTreeSet<String>, &'static str> {
    let Some(root) = inventory_root.to_str() else {
        return Err("file inventory root was not valid Unicode");
    };
    let output = run_git_in_dir(&["ls-files", "-z"], root)
        .map_err(|_| "tracked file inventory was unavailable")?;
    if output.contains('\u{fffd}') {
        return Err("tracked file inventory was not valid UTF-8");
    }

    let mut inventory = BTreeSet::new();
    for (index, raw) in output
        .split('\0')
        .filter(|value| !value.is_empty())
        .enumerate()
    {
        if index >= MAX_FILE_INVENTORY_ENTRIES {
            return Err("tracked file inventory exceeded the entry limit");
        }
        let slash_normalized = raw.replace('\\', "/");
        let normalized = normalize_scope(raw);
        if normalized.is_empty()
            || normalized.chars().count() > MAX_FILE_INVENTORY_PATH_CHARS
            || slash_normalized.starts_with('/')
            || slash_normalized.as_bytes().get(1) == Some(&b':')
            || normalized.split('/').any(|component| component == "..")
            || Path::new(&normalized).is_absolute()
        {
            return Err("tracked file inventory contained an invalid path");
        }
        inventory.insert(normalized);
    }
    Ok(inventory)
}

fn attach_knowledge_intent(
    task_id: &str,
    intent: &str,
    provenance: &str,
    candidates: &BTreeSet<String>,
    fallback: bool,
    config: KnowledgeAttachmentConfig,
    knowledge_attachment_touches: &mut BTreeMap<TaskId, BTreeSet<String>>,
    provenance_by_task: &mut BTreeMap<TaskId, BTreeMap<String, BTreeSet<String>>>,
    resolution_omissions: &mut Vec<WorkGraphOmission>,
) {
    match resolve_path_intent(intent, candidates, config.parent_directory) {
        Ok(resolution) if config.enables(resolution.match_type) => {
            let task_touches = knowledge_attachment_touches
                .entry(task_id.to_string())
                .or_default();
            if task_touches.len() >= MAX_KNOWLEDGE_TOUCHES_PER_TASK
                && !task_touches.contains(&resolution.path)
            {
                if config.resolution_omissions {
                    push_knowledge_resolution_omission(
                        resolution_omissions,
                        task_id,
                        intent,
                        "knowledge touch limit was reached",
                    );
                }
                return;
            }
            task_touches.insert(resolution.path.clone());
            record_knowledge_provenance(
                provenance_by_task,
                task_id,
                &resolution.path,
                provenance,
            );
            if fallback {
                record_knowledge_provenance(
                    provenance_by_task,
                    task_id,
                    &resolution.path,
                    "fallback",
                );
            }
            if resolution.match_type == PathMatchType::ParentDirectory {
                record_knowledge_provenance(
                    provenance_by_task,
                    task_id,
                    &resolution.path,
                    "parent-directory",
                );
            }
        }
        Ok(_) => {}
        Err(PathResolutionFailure::Ambiguous) => {
            debug_assert!(
                !config.ambiguous,
                "ambiguous knowledge attachment has no deterministic target"
            );
            if config.resolution_omissions {
                push_knowledge_resolution_omission(
                    resolution_omissions,
                    task_id,
                    intent,
                    "knowledge path resolution was ambiguous",
                );
            }
        }
        Err(PathResolutionFailure::NoMatch) => {
            if config.resolution_omissions {
                push_knowledge_resolution_omission(
                    resolution_omissions,
                    task_id,
                    intent,
                    "knowledge path resolution found no tracked path",
                );
            }
        }
    }
}

fn record_knowledge_provenance(
    provenance_by_task: &mut BTreeMap<TaskId, BTreeMap<String, BTreeSet<String>>>,
    task_id: &str,
    path: &str,
    provenance: &str,
) {
    provenance_by_task
        .entry(task_id.to_string())
        .or_default()
        .entry(path.to_string())
        .or_default()
        .insert(provenance.to_string());
}

fn push_knowledge_resolution_omission(
    omissions: &mut Vec<WorkGraphOmission>,
    task_id: &str,
    intent: &str,
    detail: &str,
) {
    let mut omission = WorkGraphOmission::new(
        WorkGraphOmissionReason::ResolutionIncomplete,
        1,
        vec![format!("{task_id}: {intent}")],
    );
    omission.detail = detail.to_string();
    omissions.push(omission);
}

fn declared_contract_path_intents(node: &WorkNode) -> Vec<String> {
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
            (!raw.trim().eq_ignore_ascii_case("none")).then(|| raw.trim().to_string())
        })
        .collect()
}

pub(crate) fn harvest_contract_path_intents(node: &WorkNode) -> Vec<String> {
    let mut intents = BTreeSet::new();
    for value in node
        .contract
        .inputs
        .iter()
        .chain(node.contract.outputs.iter())
        .chain(node.contract.acceptance.iter())
    {
        let trimmed = value.trim();
        let lower = trimmed.to_ascii_lowercase();
        if ["touch:", "file:", "module:"]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            continue;
        }
        for token in value.split(|character: char| {
            character.is_ascii_whitespace()
                || matches!(
                    character,
                    ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
                )
        }) {
            let token = strip_path_token(token);
            if token.is_empty()
                || is_line_range_fragment(token)
                || token.contains("://")
                || ["touch:", "file:", "module:"]
                    .iter()
                    .any(|prefix| token.to_ascii_lowercase().starts_with(prefix))
                || !is_path_like_token(token)
            {
                continue;
            }
            if let Some(normalized) = normalize_path_reference(token) {
                intents.insert(normalized);
            }
        }
    }
    intents.into_iter().collect()
}

pub(crate) fn resolve_path_intent(
    intent: &str,
    candidates: &BTreeSet<String>,
    allow_parent_directory: bool,
) -> Result<PathResolution, PathResolutionFailure> {
    let Some(intent) = normalize_path_reference(intent) else {
        return Err(PathResolutionFailure::NoMatch);
    };
    let candidates: BTreeSet<_> = candidates
        .iter()
        .filter_map(|candidate| normalize_path_reference(candidate))
        .collect();

    if candidates.contains(&intent) {
        return Ok(PathResolution {
            path: intent,
            match_type: PathMatchType::Exact,
        });
    }

    if !intent.contains('/') {
        let matches: Vec<_> = candidates
            .iter()
            .filter(|candidate| candidate.rsplit('/').next() == Some(intent.as_str()))
            .cloned()
            .collect();
        return match matches.as_slice() {
            [path] => Ok(PathResolution {
                path: path.clone(),
                match_type: PathMatchType::UniqueBasename,
            }),
            [] => Err(PathResolutionFailure::NoMatch),
            _ => Err(PathResolutionFailure::Ambiguous),
        };
    }

    let suffix = format!("/{intent}");
    let matches: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.ends_with(&suffix))
        .cloned()
        .collect();
    match matches.as_slice() {
        [path] => {
            return Ok(PathResolution {
                path: path.clone(),
                match_type: PathMatchType::PathSuffix,
            });
        }
        [] => {}
        _ => return Err(PathResolutionFailure::Ambiguous),
    }

    if allow_parent_directory {
        let mut parent = intent.rsplit_once('/').map(|(parent, _)| parent);
        while let Some(candidate_parent) = parent {
            if candidate_parent.is_empty() {
                break;
            }
            let prefix = format!("{candidate_parent}/");
            let descendant_count = candidates
                .iter()
                .filter(|candidate| candidate.starts_with(&prefix))
                .take(MAX_KNOWLEDGE_TOUCHES_PER_TASK + 1)
                .count();
            if (1..=MAX_KNOWLEDGE_TOUCHES_PER_TASK).contains(&descendant_count) {
                return Ok(PathResolution {
                    path: candidate_parent.to_string(),
                    match_type: PathMatchType::ParentDirectory,
                });
            }
            if descendant_count > MAX_KNOWLEDGE_TOUCHES_PER_TASK {
                break;
            }
            parent = candidate_parent.rsplit_once('/').map(|(parent, _)| parent);
        }
    }

    Err(PathResolutionFailure::NoMatch)
}

fn normalize_path_reference(value: &str) -> Option<String> {
    let replaced = strip_path_token(value).replace('\\', "/");
    if replaced.starts_with('/')
        || replaced
            .as_bytes()
            .get(1)
            .is_some_and(|character| *character == b':')
    {
        return None;
    }
    let normalized = replaced
        .trim_start_matches("./")
        .trim_matches('/')
        .to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.split('/').any(|component| {
            component.is_empty() || component == "." || component == ".."
        })
    {
        return None;
    }
    Some(normalized)
}

fn strip_path_token(value: &str) -> &str {
    let token = value
        .trim()
        .trim_matches(|character| matches!(character, '`' | '"' | '\''))
        .trim_end_matches(|character| matches!(character, ',' | ';' | '.' | '!' | '?'));
    let Some((path, range)) = token.rsplit_once(':') else {
        return token;
    };
    if is_line_range(range) {
        path.trim_end_matches(|character| matches!(character, ',' | ';' | '.' | '!' | '?'))
    } else {
        token
    }
}

fn is_line_range(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(start) = parts.next() else {
        return false;
    };
    let end = parts.next();
    parts.next().is_none()
        && !start.is_empty()
        && start.chars().all(|character| character.is_ascii_digit())
        && end.map_or(true, |end| {
            !end.is_empty() && end.chars().all(|character| character.is_ascii_digit())
        })
}

fn is_line_range_fragment(value: &str) -> bool {
    value
        .strip_prefix(':')
        .is_some_and(is_line_range)
}

fn is_path_like_token(value: &str) -> bool {
    if value.contains('/') || value.contains('\\') {
        return true;
    }
    let value = strip_path_token(value);
    value.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && !extension.is_empty()
            && extension
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedGotcha {
    pub id: String,
    pub scope: Vec<String>,
    pub summary: String,
    pub source_ref: String,
    pub fingerprint_ref: String,
    pub source_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextHubLint {
    pub context_node_id: TaskId,
    pub linked_task_ids: Vec<TaskId>,
    pub task_fraction: f64,
    pub threshold: f64,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeSourceFingerprint {
    pub source_ref: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextDerivationReport {
    pub gotchas: Vec<DerivedGotcha>,
    pub hub_lints: Vec<ContextHubLint>,
    pub source_fingerprints: Vec<KnowledgeSourceFingerprint>,
    pub knowledge_available: bool,
    pub touches_available: bool,
    pub knowledge_edge_count: usize,
}

/// Read project knowledge from `project_path`, rebuild derived nodes, and add
/// scoped knowledge edges. This function never writes `.ai-docs` or the wiki.
pub fn derive_project_context<R: TouchesResolver>(
    graph: &mut TaskGraph,
    project_path: &Path,
    resolver: &R,
) -> ContextDerivationReport {
    clear_derived_context(graph);
    let load = load_project_knowledge(
        project_path,
        None,
        None,
        KnowledgeAttachmentConfig::production(),
    );
    graph.omissions.extend(load.omissions.clone());
    if !load.available {
        return ContextDerivationReport {
            gotchas: Vec::new(),
            hub_lints: Vec::new(),
            source_fingerprints: load.fingerprints,
            knowledge_available: false,
            touches_available: false,
            knowledge_edge_count: 0,
        };
    }

    let coverage = match resolver.resolve_touches(graph) {
        Ok(coverage) if coverage.available => coverage,
        Ok(_) => {
            graph.omissions.push(WorkGraphOmission::new(
                WorkGraphOmissionReason::CodegraphUnavailable,
                1,
                vec!["touches-resolver".to_string()],
            ));
            append_context_nodes(graph, &load.gotchas, &load.inferred_scope_provenance);
            return ContextDerivationReport {
                gotchas: load.gotchas,
                hub_lints: Vec::new(),
                source_fingerprints: load.fingerprints,
                knowledge_available: true,
                touches_available: false,
                knowledge_edge_count: 0,
            };
        }
        Err(error) => {
            graph.omissions.push(WorkGraphOmission::new(
                WorkGraphOmissionReason::SourceUnreadable,
                1,
                vec![format!("touches-resolver: {error}")],
            ));
            append_context_nodes(graph, &load.gotchas, &load.inferred_scope_provenance);
            return ContextDerivationReport {
                gotchas: load.gotchas,
                hub_lints: Vec::new(),
                source_fingerprints: load.fingerprints,
                knowledge_available: true,
                touches_available: false,
                knowledge_edge_count: 0,
            };
        }
    };

    derive_loaded_context(
        graph,
        load,
        &coverage,
        &coverage.touches,
        &BTreeMap::new(),
    )
}

pub(crate) fn derive_project_context_from_knowledge(
    graph: &mut TaskGraph,
    project_path: &Path,
    institutional_wiki_root: Option<&Path>,
    declared_coverage: &TouchCoverageReport,
    knowledge: &KnowledgeTouchCoverage,
    scope_candidates: Option<&BTreeSet<String>>,
    config: KnowledgeAttachmentConfig,
) -> ContextDerivationReport {
    clear_derived_context(graph);
    let load = load_project_knowledge(
        project_path,
        institutional_wiki_root,
        scope_candidates,
        config,
    );
    graph.omissions.extend(load.omissions.clone());
    if !load.available {
        return ContextDerivationReport {
            gotchas: Vec::new(),
            hub_lints: Vec::new(),
            source_fingerprints: load.fingerprints,
            knowledge_available: false,
            touches_available: false,
            knowledge_edge_count: 0,
        };
    }

    let expanded_available = declared_coverage.available
        || (config.file_inventory_fallback && scope_candidates.is_some());
    if !expanded_available {
        graph.omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::CodegraphUnavailable,
            1,
            vec!["touches-resolver".to_string()],
        ));
        append_context_nodes(graph, &load.gotchas, &load.inferred_scope_provenance);
        return ContextDerivationReport {
            gotchas: load.gotchas,
            hub_lints: Vec::new(),
            source_fingerprints: load.fingerprints,
            knowledge_available: true,
            touches_available: false,
            knowledge_edge_count: 0,
        };
    }

    derive_loaded_context(
        graph,
        load,
        declared_coverage,
        &knowledge.knowledge_attachment_touches,
        &knowledge.provenance_by_task,
    )
}

fn derive_loaded_context(
    graph: &mut TaskGraph,
    load: KnowledgeLoad,
    declared_coverage: &TouchCoverageReport,
    expanded_touches: &BTreeMap<TaskId, BTreeSet<String>>,
    provenance_by_task: &BTreeMap<TaskId, BTreeMap<String, BTreeSet<String>>>,
) -> ContextDerivationReport {
    append_context_nodes(graph, &load.gotchas, &load.inferred_scope_provenance);
    let all_task_ids: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Task)
        .map(|node| node.id.clone())
        .collect();
    let missing_task_ids = declared_coverage.unresolved_task_ids.clone();
    if !missing_task_ids.is_empty() {
        graph.omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::ResolutionIncomplete,
            missing_task_ids.len(),
            missing_task_ids
                .iter()
                .take(MAX_OMISSION_EXAMPLES)
                .cloned()
                .collect(),
        ));
    }
    // Anti-hub fractions use the full planned Task universe. A partial
    // resolver must not shrink the denominator until generic context appears
    // discriminating; missing facts remain visible through the omission above.
    let task_ids = all_task_ids;
    let mut base_candidates: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
    let mut expanded_candidates: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
    for gotcha in &load.gotchas {
        let context_id = context_node_id(&gotcha.id);
        let base_scope: &[String] = if load.inferred_scope_provenance.contains_key(&gotcha.id) {
            &[]
        } else {
            &gotcha.scope
        };
        for task_id in &task_ids {
            if declared_coverage
                .touches
                .get(task_id)
                .is_some_and(|touches| scope_intersects(base_scope, touches))
            {
                base_candidates
                    .entry(context_id.clone())
                    .or_default()
                    .push(task_id.clone());
            }
            if expanded_touches
                .get(task_id)
                .is_some_and(|touches| scope_intersects(&gotcha.scope, touches))
            {
                expanded_candidates
                    .entry(context_id.clone())
                    .or_default()
                    .push(task_id.clone());
            }
        }
    }

    let mut hub_lints = Vec::new();
    let mut knowledge_edge_count = 0;
    for gotcha in &load.gotchas {
        let context_id = context_node_id(&gotcha.id);
        let mut base_linked = base_candidates.remove(&context_id).unwrap_or_default();
        let mut expanded_linked = expanded_candidates.remove(&context_id).unwrap_or_default();
        // `*` declares repo-wide applicability. Even partial or successfully
        // empty touch facts cannot make that declaration task-specific, so
        // lint it against the complete plan while still adding no edges.
        if gotcha.scope.iter().any(|scope| scope == "*")
            && task_ids.len() >= ANTI_HUB_MIN_TASKS
        {
            base_linked = task_ids.clone();
            expanded_linked = task_ids.clone();
        }
        let base_fraction = if task_ids.is_empty() {
            0.0
        } else {
            base_linked.len() as f64 / task_ids.len() as f64
        };
        if task_ids.len() >= ANTI_HUB_MIN_TASKS
            && base_fraction >= ANTI_HUB_TASK_FRACTION
        {
            hub_lints.push(ContextHubLint {
                context_node_id: context_id,
                linked_task_ids: base_linked,
                task_fraction: base_fraction,
                threshold: ANTI_HUB_TASK_FRACTION,
                detail: "context applies to a high fraction of tasks; move standing guidance to the role prompt or narrow its scope".to_string(),
            });
            continue;
        }
        for task_id in &base_linked {
            graph.edges.push(
                WorkEdge::new(
                    &context_id,
                    task_id,
                    EdgeKind::Informs,
                    EdgeProvenance::Knowledge,
                )
                .with_rationale("task touches a module in this gotcha's scope"),
            );
            knowledge_edge_count += 1;
        }

        let expanded_fraction = if task_ids.is_empty() {
            0.0
        } else {
            expanded_linked.len() as f64 / task_ids.len() as f64
        };
        if task_ids.len() >= ANTI_HUB_MIN_TASKS
            && expanded_fraction >= ANTI_HUB_TASK_FRACTION
        {
            if expanded_linked != base_linked {
                hub_lints.push(ContextHubLint {
                    context_node_id: context_id,
                    linked_task_ids: expanded_linked,
                    task_fraction: expanded_fraction,
                    threshold: ANTI_HUB_TASK_FRACTION,
                    detail: "expanded knowledge coverage applies to a high fraction of tasks; base declared-scope edges were preserved and new inferred edges were withheld".to_string(),
                });
            }
            continue;
        }
        let base_set: BTreeSet<_> = base_linked.into_iter().collect();
        for task_id in expanded_linked.drain(..) {
            if base_set.contains(&task_id) {
                continue;
            }
            graph.edges.push(
                WorkEdge::new(
                    &context_id,
                    &task_id,
                    EdgeKind::Informs,
                    EdgeProvenance::Knowledge,
                )
                .with_rationale(expanded_edge_rationale(
                    gotcha,
                    &task_id,
                    expanded_touches,
                    provenance_by_task,
                    &load.inferred_scope_provenance,
                )),
            );
            knowledge_edge_count += 1;
        }
    }

    ContextDerivationReport {
        gotchas: load.gotchas,
        hub_lints,
        source_fingerprints: load.fingerprints,
        knowledge_available: true,
        touches_available: true,
        knowledge_edge_count,
    }
}

/// Attach context from the exact coverage report already produced by the
/// preceding touch-derivation stage; no second analyzer call or inference.
pub fn derive_project_context_from_coverage(
    graph: &mut TaskGraph,
    project_path: &Path,
    coverage: &TouchCoverageReport,
) -> ContextDerivationReport {
    struct BorrowedCoverage<'a>(&'a TouchCoverageReport);
    impl TouchesResolver for BorrowedCoverage<'_> {
        fn resolve_touches(
            &self,
            _graph: &TaskGraph,
        ) -> Result<TouchCoverageReport, String> {
            Ok(self.0.clone())
        }
    }
    derive_project_context(graph, project_path, &BorrowedCoverage(coverage))
}

/// A real adapter for the WS-7 `GotchaAttachmentProvider` seam. The provider
/// derives scoped graph context on demand and returns only non-hub task
/// attachments. Missing project knowledge remains `Ok(None)`.
pub struct ProjectKnowledgeGotchaProvider<'a, R> {
    pub graph: &'a TaskGraph,
    pub resolver: R,
}

impl<R: TouchesResolver> GotchaAttachmentProvider for ProjectKnowledgeGotchaProvider<'_, R> {
    fn gotchas(&self, project_path: &Path) -> Result<Option<Vec<GotchaAttachment>>, String> {
        let mut graph = (*self.graph).clone();
        let report = derive_project_context(&mut graph, project_path, &self.resolver);
        if !report.knowledge_available {
            return Ok(None);
        }
        if !report.touches_available {
            return Err("touches resolver unavailable; scoped gotchas cannot be resolved".to_string());
        }
        let summaries: BTreeMap<_, _> = report
            .gotchas
            .iter()
            .map(|gotcha| (context_node_id(&gotcha.id), gotcha.summary.clone()))
            .collect();
        let attachments = graph
            .edges
            .iter()
            .filter(|edge| {
                edge.kind == EdgeKind::Informs && edge.provenance == EdgeProvenance::Knowledge
            })
            .filter_map(|edge| {
                summaries.get(&edge.source).map(|summary| GotchaAttachment {
                    lane_id: edge.target.clone(),
                    acceptance: summary.clone(),
                })
            })
            .collect();
        Ok(Some(attachments))
    }
}

/// Compare a derived node's recorded source hash with a fresh fingerprint.
pub fn context_node_is_stale(
    node: &WorkNode,
    fingerprints: &[KnowledgeSourceFingerprint],
) -> bool {
    let Some(expansion) = node.expansion.as_ref() else {
        return false;
    };
    if expansion.template != DERIVED_CONTEXT_TEMPLATE {
        return false;
    }
    let Some(fingerprint_ref) = expansion.parameters.get("fingerprint_ref") else {
        return true;
    };
    let Some(recorded_hash) = expansion.parameters.get("source_hash") else {
        return true;
    };
    fingerprints
        .iter()
        .find(|fingerprint| fingerprint_ref == &fingerprint.source_ref)
        .map_or(true, |fingerprint| {
            fingerprint.content_hash != *recorded_hash
        })
}

struct KnowledgeLoad {
    gotchas: Vec<DerivedGotcha>,
    inferred_scope_provenance: BTreeMap<String, BTreeMap<String, String>>,
    fingerprints: Vec<KnowledgeSourceFingerprint>,
    omissions: Vec<WorkGraphOmission>,
    available: bool,
}

fn load_project_knowledge(
    project_path: &Path,
    institutional_wiki_root: Option<&Path>,
    scope_candidates: Option<&BTreeSet<String>>,
    config: KnowledgeAttachmentConfig,
) -> KnowledgeLoad {
    let ai_docs = project_path.join(".ai-docs");
    if !ai_docs.is_dir() {
        return KnowledgeLoad {
            gotchas: Vec::new(),
            inferred_scope_provenance: BTreeMap::new(),
            fingerprints: Vec::new(),
            omissions: vec![WorkGraphOmission::new(
                WorkGraphOmissionReason::ProjectKnowledgeUnavailable,
                1,
                vec![ai_docs.display().to_string()],
            )],
            available: false,
        };
    }

    let mut gotchas = Vec::new();
    let mut inferred_scope_provenance = BTreeMap::new();
    let mut fingerprints = Vec::new();
    let mut omissions = Vec::new();
    let curated_line_limit = load_curated_line_limit(
        &ai_docs,
        &mut fingerprints,
        &mut omissions,
    );
    for filename in ["project-dna.md", "bug-patterns.md", "learnings.jsonl"] {
        let path = ai_docs.join(filename);
        match fs::read_to_string(&path) {
            Ok(content) => {
                let source_hash = stable_hash(content.as_bytes());
                if filename.ends_with(".jsonl") {
                    if let Some(line_limit) = curated_line_limit {
                        let curated_hash = stable_hash(
                            format!("{source_hash}:last-curated-line={line_limit}").as_bytes(),
                        );
                        fingerprints.push(KnowledgeSourceFingerprint {
                            source_ref: format!(".ai-docs/{filename}"),
                            content_hash: curated_hash.clone(),
                        });
                        parse_learnings(
                            filename,
                            &content,
                            &curated_hash,
                            line_limit,
                            &mut gotchas,
                            &mut omissions,
                        );
                    } else {
                        fingerprints.push(KnowledgeSourceFingerprint {
                            source_ref: format!(".ai-docs/{filename}"),
                            content_hash: source_hash,
                        });
                    }
                } else {
                    fingerprints.push(KnowledgeSourceFingerprint {
                        source_ref: format!(".ai-docs/{filename}"),
                        content_hash: source_hash.clone(),
                    });
                    parse_markdown(
                        filename,
                        &content,
                        &source_hash,
                        institutional_wiki_root,
                        scope_candidates,
                        config,
                        &mut gotchas,
                        &mut inferred_scope_provenance,
                        &mut omissions,
                    );
                }
            }
            Err(error) => omissions.push(WorkGraphOmission::new(
                if error.kind() == std::io::ErrorKind::NotFound {
                    WorkGraphOmissionReason::ProjectKnowledgeUnavailable
                } else {
                    WorkGraphOmissionReason::SourceUnreadable
                },
                1,
                vec![format!("{}: {error}", path.display())],
            )),
        }
    }

    let mut dropped_scope_count = 0;
    let mut dropped_scope_sources = BTreeSet::new();
    for gotcha in &mut gotchas {
        gotcha.scope.sort();
        gotcha.scope.dedup();
        let original_len = gotcha.scope.len();
        gotcha
            .scope
            .retain(|scope| scope.chars().count() <= MAX_CONTEXT_SCOPE_CHARS);
        if gotcha.scope.len() > MAX_CONTEXT_SCOPES_PER_GOTCHA {
            gotcha.scope.truncate(MAX_CONTEXT_SCOPES_PER_GOTCHA);
        }
        if let Some(provenance) = inferred_scope_provenance.get_mut(&gotcha.id) {
            provenance.retain(|scope, _| gotcha.scope.contains(scope));
        }
        let dropped = original_len - gotcha.scope.len();
        if dropped > 0 {
            dropped_scope_count += dropped;
            dropped_scope_sources.insert(gotcha.source_ref.clone());
        }
    }
    if dropped_scope_count > 0 {
        omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::ResolutionIncomplete,
            dropped_scope_count,
            dropped_scope_sources
                .into_iter()
                .take(MAX_OMISSION_EXAMPLES)
                .collect(),
        ));
    }

    // The real project DNA has many unscoped entries, while curated learnings
    // carry the strongest file/module evidence. Prioritize attachable records
    // so source order cannot consume the bounded context budget first.
    gotchas.sort_by(|left, right| {
        left.scope
            .is_empty()
            .cmp(&right.scope.is_empty())
            .then_with(|| left.source_ref.cmp(&right.source_ref))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut seen_ids = BTreeSet::new();
    gotchas.retain(|gotcha| seen_ids.insert(gotcha.id.clone()));
    if gotchas.len() > MAX_DERIVED_CONTEXT_NODES {
        let omitted = gotchas.len() - MAX_DERIVED_CONTEXT_NODES;
        gotchas.truncate(MAX_DERIVED_CONTEXT_NODES);
        omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::ResolutionIncomplete,
            omitted,
            vec![format!("context node cap {MAX_DERIVED_CONTEXT_NODES}")],
        ));
    }

    KnowledgeLoad {
        gotchas,
        inferred_scope_provenance,
        fingerprints,
        omissions,
        available: true,
    }
}

fn load_curated_line_limit(
    ai_docs: &Path,
    fingerprints: &mut Vec<KnowledgeSourceFingerprint>,
    omissions: &mut Vec<WorkGraphOmission>,
) -> Option<usize> {
    let path = ai_docs.join("curation-state.json");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) => {
            omissions.push(WorkGraphOmission::new(
                if error.kind() == std::io::ErrorKind::NotFound {
                    WorkGraphOmissionReason::ProjectKnowledgeUnavailable
                } else {
                    WorkGraphOmissionReason::SourceUnreadable
                },
                1,
                vec![format!("{}: {error}", path.display())],
            ));
            return None;
        }
    };
    fingerprints.push(KnowledgeSourceFingerprint {
        source_ref: ".ai-docs/curation-state.json".to_string(),
        content_hash: stable_hash(content.as_bytes()),
    });
    match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(value) => match value.get("last_curated_line").and_then(|value| value.as_u64()) {
            Some(line) => usize::try_from(line).ok().or_else(|| {
                omissions.push(WorkGraphOmission::new(
                    WorkGraphOmissionReason::ResolutionIncomplete,
                    1,
                    vec![".ai-docs/curation-state.json:last_curated_line".to_string()],
                ));
                None
            }),
            None => {
                omissions.push(WorkGraphOmission::new(
                    WorkGraphOmissionReason::ResolutionIncomplete,
                    1,
                    vec![".ai-docs/curation-state.json:last_curated_line".to_string()],
                ));
                None
            }
        },
        Err(error) => {
            omissions.push(WorkGraphOmission::new(
                WorkGraphOmissionReason::SourceUnreadable,
                1,
                vec![format!("{}: {error}", path.display())],
            ));
            None
        }
    }
}

fn parse_markdown(
    filename: &str,
    content: &str,
    source_hash: &str,
    institutional_wiki_root: Option<&Path>,
    scope_candidates: Option<&BTreeSet<String>>,
    config: KnowledgeAttachmentConfig,
    gotchas: &mut Vec<DerivedGotcha>,
    inferred_scope_provenance: &mut BTreeMap<String, BTreeMap<String, String>>,
    omissions: &mut Vec<WorkGraphOmission>,
) {
    let mut heading = String::new();
    let mut start_line = 1;
    let mut body = Vec::new();
    let mut in_fence = false;
    for (index, line) in content.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(next_heading) = markdown_heading(line) {
            flush_markdown_section(
                filename,
                &heading,
                start_line,
                &body,
                source_hash,
                institutional_wiki_root,
                scope_candidates,
                config,
                gotchas,
                inferred_scope_provenance,
                omissions,
            );
            heading = next_heading;
            start_line = index + 1;
            body.clear();
        } else if !heading.is_empty() {
            body.push(line.to_string());
        }
    }
    flush_markdown_section(
        filename,
        &heading,
        start_line,
        &body,
        source_hash,
        institutional_wiki_root,
        scope_candidates,
        config,
        gotchas,
        inferred_scope_provenance,
        omissions,
    );
}

fn markdown_heading(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let hashes = trimmed.chars().take_while(|character| *character == '#').count();
    if !(2..=4).contains(&hashes) {
        return None;
    }
    Some(trimmed[hashes..].trim().to_string())
}

fn flush_markdown_section(
    filename: &str,
    heading: &str,
    start_line: usize,
    body: &[String],
    source_hash: &str,
    institutional_wiki_root: Option<&Path>,
    scope_candidates: Option<&BTreeSet<String>>,
    config: KnowledgeAttachmentConfig,
    gotchas: &mut Vec<DerivedGotcha>,
    inferred_scope_provenance: &mut BTreeMap<String, BTreeMap<String, String>>,
    omissions: &mut Vec<WorkGraphOmission>,
) {
    if heading.is_empty()
        || matches!(heading.to_ascii_lowercase().as_str(), "template" | "bugs")
        || body.iter().all(|line| line.trim().is_empty())
    {
        return;
    }
    let mut scope = markdown_scope(body);
    let summary = markdown_summary(heading, body);
    if summary.is_empty() {
        return;
    }
    let source_ref = format!(".ai-docs/{filename}#L{start_line}");
    let inferred = if config.inferred_scope
        && !has_explicit_markdown_scope(body)
        && scope_candidates.is_some()
    {
        resolve_inferred_scope(
            body,
            &source_ref,
            scope_candidates.expect("scope candidates checked above"),
            config,
            omissions,
        )
    } else {
        BTreeMap::new()
    };
    scope.extend(inferred.keys().cloned());
    scope.sort();
    scope.dedup();
    let gotcha_id = stable_hash(format!("{source_ref}:{heading}").as_bytes());
    gotchas.push(DerivedGotcha {
        id: gotcha_id.clone(),
        scope: scope.clone(),
        summary,
        fingerprint_ref: format!(".ai-docs/{filename}"),
        source_ref,
        source_hash: source_hash.to_string(),
    });
    if !inferred.is_empty() {
        inferred_scope_provenance.insert(gotcha_id, inferred.clone());
    }

    for line in body {
        if let Some(global_ref) = global_cross_ref(line) {
            let source_ref = format!("global:{global_ref}");
            let gotcha_id = stable_hash(format!("{source_ref}:{heading}").as_bytes());
            gotchas.push(DerivedGotcha {
                id: gotcha_id.clone(),
                scope: scope.clone(),
                summary: institutional_wiki_root
                    .and_then(|root| read_global_summary(root, &global_ref))
                    .unwrap_or_else(|| {
                        bound_summary(&format!("{heading}: institutional context"))
                    }),
                fingerprint_ref: format!(".ai-docs/{filename}"),
                source_ref,
                source_hash: source_hash.to_string(),
            });
            if !inferred.is_empty() {
                inferred_scope_provenance.insert(gotcha_id, inferred.clone());
            }
        }
    }
}

fn has_explicit_markdown_scope(body: &[String]) -> bool {
    body.iter().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("**scope**")
            || lower.contains("**files**")
            || lower.trim_start().starts_with("scope:")
            || lower.trim_start().starts_with("files:")
    })
}

fn resolve_inferred_scope(
    body: &[String],
    source_ref: &str,
    candidates: &BTreeSet<String>,
    config: KnowledgeAttachmentConfig,
    omissions: &mut Vec<WorkGraphOmission>,
) -> BTreeMap<String, String> {
    let mut resolved = BTreeMap::new();
    let mut local_omissions = Vec::new();
    for raw in body
        .iter()
        .flat_map(|line| code_spans(line))
        .filter(|value| is_path_like_token(value))
    {
        match resolve_path_intent(&raw, candidates, false) {
            Ok(resolution) if config.enables(resolution.match_type) => {
                if resolved.len() >= MAX_CONTEXT_SCOPES_PER_GOTCHA
                    && !resolved.contains_key(&resolution.path)
                {
                    push_inferred_scope_omission(
                        &mut local_omissions,
                        source_ref,
                        &raw,
                        "inferred knowledge scope exceeded the scope limit",
                    );
                    continue;
                }
                resolved.insert(
                    resolution.path,
                    format!("inferred-scope:{}", match_type_label(resolution.match_type)),
                );
            }
            Ok(_) => {}
            Err(PathResolutionFailure::Ambiguous) if config.resolution_omissions => {
                push_inferred_scope_omission(
                    &mut local_omissions,
                    source_ref,
                    &raw,
                    "inferred knowledge scope was ambiguous",
                );
            }
            Err(PathResolutionFailure::NoMatch) if config.resolution_omissions => {
                push_inferred_scope_omission(
                    &mut local_omissions,
                    source_ref,
                    &raw,
                    "inferred knowledge scope found no tracked path",
                );
            }
            Err(_) => {}
        }
    }
    omissions.extend(aggregate_resolution_omissions(local_omissions));
    resolved
}

fn push_inferred_scope_omission(
    omissions: &mut Vec<WorkGraphOmission>,
    source_ref: &str,
    raw: &str,
    detail: &str,
) {
    let mut omission = WorkGraphOmission::new(
        WorkGraphOmissionReason::ResolutionIncomplete,
        1,
        vec![format!("{source_ref}: {}", strip_path_token(raw))],
    );
    omission.detail = detail.to_string();
    omissions.push(omission);
}

fn match_type_label(match_type: PathMatchType) -> &'static str {
    match match_type {
        PathMatchType::Exact => "exact",
        PathMatchType::UniqueBasename => "unique-basename",
        PathMatchType::PathSuffix => "path-suffix",
        PathMatchType::ParentDirectory => "parent-directory",
    }
}

fn read_global_summary(institutional_wiki_root: &Path, global_ref: &str) -> Option<String> {
    let root = institutional_wiki_root.canonicalize().ok()?;
    let candidate = root.join(global_ref).canonicalize().ok()?;
    if !candidate.starts_with(&root) || !candidate.is_file() {
        return None;
    }
    let content = fs::read_to_string(candidate).ok()?;
    let (frontmatter, body) = split_frontmatter(&content);
    let title = frontmatter_field(frontmatter, "title").or_else(|| first_h1(body))?;
    let description = frontmatter_field(frontmatter, "description")?;
    Some(bound_summary(&format!("{title}: {description}")))
}

fn markdown_scope(body: &[String]) -> Vec<String> {
    let mut scopes = BTreeSet::new();
    for line in body {
        let lower = line.to_ascii_lowercase();
        let explicit = lower.contains("**scope**")
            || lower.contains("**files**")
            || lower.trim_start().starts_with("scope:");
        if explicit {
            let code_values = code_spans(line);
            if code_values.is_empty() {
                let plain = line.replace("**", "");
                if let Some((_, value)) = plain.split_once(':') {
                    for token in value.split(',') {
                        scopes.insert(normalize_scope(token));
                    }
                }
            } else {
                for token in code_values {
                    scopes.insert(normalize_scope(&token));
                }
            }
        }
    }
    scopes.into_iter().filter(|scope| !scope.is_empty()).collect()
}

fn markdown_summary(heading: &str, body: &[String]) -> String {
    for line in body {
        let trimmed = line.trim().trim_start_matches('-').trim();
        if trimmed.is_empty()
            || trimmed.starts_with("```")
            || trimmed.contains("-> global:")
            || trimmed.to_ascii_lowercase().contains("**scope**")
            || trimmed.to_ascii_lowercase().contains("**files**")
        {
            continue;
        }
        return bound_summary(&format!("{heading}: {}", strip_markdown_label(trimmed)));
    }
    bound_summary(heading)
}

fn parse_learnings(
    filename: &str,
    content: &str,
    source_hash: &str,
    line_limit: usize,
    gotchas: &mut Vec<DerivedGotcha>,
    omissions: &mut Vec<WorkGraphOmission>,
) {
    for (index, line) in content.lines().take(line_limit).enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                omissions.push(WorkGraphOmission::new(
                    WorkGraphOmissionReason::SourceUnreadable,
                    1,
                    vec![format!(".ai-docs/{filename}#L{}: {error}", index + 1)],
                ));
                continue;
            }
        };
        let Some(insight) = value.get("insight").and_then(|value| value.as_str()) else {
            continue;
        };
        let scope: Vec<_> = value
            .get("files_touched")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str())
            .map(normalize_scope)
            .filter(|scope| !scope.is_empty())
            .collect();
        let source_ref = format!(".ai-docs/{filename}#L{}", index + 1);
        let stable_id = value
            .get("id")
            .and_then(|value| value.as_str())
            .map(ToString::to_string)
            .unwrap_or_else(|| stable_hash(line.as_bytes()));
        gotchas.push(DerivedGotcha {
            id: stable_id,
            scope,
            summary: bound_summary(insight),
            fingerprint_ref: format!(".ai-docs/{filename}"),
            source_ref,
            source_hash: source_hash.to_string(),
        });
    }
}

fn append_context_nodes(
    graph: &mut TaskGraph,
    gotchas: &[DerivedGotcha],
    inferred_scope_provenance: &BTreeMap<String, BTreeMap<String, String>>,
) {
    for gotcha in gotchas {
        let id = context_node_id(&gotcha.id);
        let mut parameters = BTreeMap::new();
        parameters.insert("scope".to_string(), gotcha.scope.join(","));
        parameters.insert("summary".to_string(), gotcha.summary.clone());
        parameters.insert("source_ref".to_string(), gotcha.source_ref.clone());
        parameters.insert(
            "fingerprint_ref".to_string(),
            gotcha.fingerprint_ref.clone(),
        );
        parameters.insert("source_hash".to_string(), gotcha.source_hash.clone());
        if let Some(provenance) = inferred_scope_provenance.get(&gotcha.id) {
            parameters.insert(
                "knowledge_provenance".to_string(),
                provenance
                    .iter()
                    .map(|(path, label)| format!("{path}={label}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        let mut node = WorkNode::new(
            id,
            NodeKind::Context,
            &gotcha.summary,
            NodeContract {
                inputs: vec![gotcha.source_ref.clone()],
                outputs: Vec::new(),
                acceptance: Vec::new(),
            },
            BindingRef::Zone("knowledge".to_string()),
            NodeStatus::Completed,
        );
        node.expansion = Some(CompositeExpansion {
            template: DERIVED_CONTEXT_TEMPLATE.to_string(),
            parameters,
        });
        graph.nodes.push(node);
    }
}

fn clear_derived_context(graph: &mut TaskGraph) {
    let removed: BTreeSet<_> = graph
        .nodes
        .iter()
        .filter(|node| {
            node.expansion
                .as_ref()
                .is_some_and(|expansion| expansion.template == DERIVED_CONTEXT_TEMPLATE)
        })
        .map(|node| node.id.clone())
        .collect();
    graph.nodes.retain(|node| !removed.contains(&node.id));
    graph
        .edges
        .retain(|edge| !removed.contains(&edge.source) && !removed.contains(&edge.target));
}

fn scope_intersects(scope: &[String], touches: &BTreeSet<String>) -> bool {
    scope.iter().any(|scope_item| {
        scope_item == "*"
            || touches.iter().any(|touch| {
                let touch = normalize_scope(touch);
                scope_item == &touch
                    || touch.starts_with(&format!("{scope_item}/"))
                    || scope_item.starts_with(&format!("{touch}/"))
            })
    })
}

fn normalize_scope(scope: &str) -> String {
    let normalized = scope
        .trim()
        .trim_matches(|character| matches!(character, '`' | '"' | '\'' | ','))
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string();
    normalized.trim_matches('/').to_ascii_lowercase()
}

fn code_spans(line: &str) -> Vec<String> {
    line.split('`')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, value)| value.to_string())
        .collect()
}

fn global_cross_ref(line: &str) -> Option<String> {
    let marker = line.find("-> global:")? + "-> global:".len();
    let tail = line[marker..].trim();
    let raw = if let Some(start) = tail.find("](") {
        let start = start + 2;
        let end = tail[start..].find(')')? + start;
        &tail[start..end]
    } else {
        tail
    };
    for category in [
        "agents/",
        "operations/",
        "patterns/",
        "practices/",
        "research/",
        "tools/",
    ] {
        let Some(start) = raw.find(category) else {
            continue;
        };
        let rest = &raw[start..];
        let end = rest.find(".md")? + 3;
        return Some(rest[..end].to_string());
    }
    None
}

fn strip_markdown_label(value: &str) -> String {
    value
        .replace("**", "")
        .trim_start_matches(|character: char| character.is_ascii_punctuation())
        .trim()
        .to_string()
}

fn bound_summary(summary: &str) -> String {
    let mut chars = summary.chars();
    let bounded: String = chars.by_ref().take(MAX_CONTEXT_SUMMARY_CHARS).collect();
    if chars.next().is_some() {
        let mut shortened: String = bounded
            .chars()
            .take(MAX_CONTEXT_SUMMARY_CHARS.saturating_sub(1))
            .collect();
        shortened.push('…');
        shortened
    } else {
        bounded
    }
}

fn context_node_id(gotcha_id: &str) -> TaskId {
    format!("context::knowledge::{gotcha_id}")
}

/// Deterministic FNV-1a fingerprint. This is a stale-content detector, not a
/// cryptographic integrity primitive.
fn stable_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn expanded_edge_rationale(
    gotcha: &DerivedGotcha,
    task_id: &str,
    expanded_touches: &BTreeMap<TaskId, BTreeSet<String>>,
    provenance_by_task: &BTreeMap<TaskId, BTreeMap<String, BTreeSet<String>>>,
    inferred_scope_provenance: &BTreeMap<String, BTreeMap<String, String>>,
) -> String {
    let mut labels = BTreeSet::new();
    let Some(task_touches) = expanded_touches.get(task_id) else {
        return "knowledge attachment matched expanded scope".to_string();
    };
    for scope in &gotcha.scope {
        for touch in task_touches {
            if !scope_intersects(std::slice::from_ref(scope), &BTreeSet::from([touch.clone()])) {
                continue;
            }
            if let Some(label) = inferred_scope_provenance
                .get(&gotcha.id)
                .and_then(|paths| paths.get(scope))
            {
                labels.insert(label.clone());
            }
            if let Some(path_labels) = provenance_by_task
                .get(task_id)
                .and_then(|paths| paths.get(touch))
            {
                labels.extend(path_labels.iter().cloned());
            }
        }
    }
    if labels.is_empty() {
        "knowledge attachment matched expanded scope".to_string()
    } else {
        format!("knowledge attachment matched {}", labels.into_iter().collect::<Vec<_>>().join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_knowledge_touch_coverage, harvest_contract_path_intents,
        derive_project_context_from_knowledge, resolve_path_intent,
        KnowledgeAttachmentConfig, KnowledgeTouchCoverage, PathMatchType,
        PathResolution, PathResolutionFailure, TouchCoverageReport, TouchesResolver,
        MAX_KNOWLEDGE_TOUCHES_PER_TASK,
    };
    use crate::orchestrator::work_graph::{
        BindingRef, EdgeKind, NodeContract, NodeKind, NodeStatus, TaskGraph,
        WorkGraphOmissionReason, WorkNode,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use tempfile::TempDir;

    #[derive(Clone)]
    struct CandidateResolver {
        coverage: TouchCoverageReport,
        candidates: BTreeSet<String>,
    }

    impl TouchesResolver for CandidateResolver {
        fn resolve_touches(
            &self,
            _graph: &TaskGraph,
        ) -> Result<TouchCoverageReport, String> {
            Ok(self.coverage.clone())
        }

        fn knowledge_candidates(&self) -> Option<BTreeSet<String>> {
            Some(self.candidates.clone())
        }
    }

    #[test]
    fn shared_path_resolver_orders_match_types_and_strips_line_ranges() {
        let candidates = BTreeSet::from([
            "src/exact.rs".to_string(),
            "src-tauri/src/http/handlers/queue.rs".to_string(),
            "src-tauri/src/orchestrator/work_graph/runtime.rs".to_string(),
            "src-tauri/src/session/completion_ledger.rs".to_string(),
            "src-tauri/src/storage/queue.rs".to_string(),
            "src/fix.rs".to_string(),
        ]);

        assert_eq!(
            resolve_path_intent("src/exact.rs:79-81,", &candidates, false),
            Ok(PathResolution {
                path: "src/exact.rs".to_string(),
                match_type: PathMatchType::Exact,
            })
        );
        assert_eq!(
            resolve_path_intent("runtime.rs:1371-1408", &candidates, false),
            Ok(PathResolution {
                path: "src-tauri/src/orchestrator/work_graph/runtime.rs".to_string(),
                match_type: PathMatchType::UniqueBasename,
            })
        );
        assert_eq!(
            resolve_path_intent(
                "session/completion_ledger.rs:69-78",
                &candidates,
                false,
            ),
            Ok(PathResolution {
                path: "src-tauri/src/session/completion_ledger.rs".to_string(),
                match_type: PathMatchType::PathSuffix,
            })
        );
        assert_eq!(
            resolve_path_intent("queue.rs:79-81", &candidates, false),
            Err(PathResolutionFailure::Ambiguous)
        );
        assert_eq!(
            resolve_path_intent("x.rs", &candidates, false),
            Err(PathResolutionFailure::NoMatch),
            "suffix matching must respect path-component boundaries"
        );
    }

    #[test]
    fn contract_harvest_uses_real_range_bearing_strings() {
        let node = task(
            "T1",
            NodeContract {
                inputs: vec!["queue.rs:79-81".to_string()],
                outputs: vec![
                    "runtime.rs:1371-1408 and :1046-1054".to_string(),
                    "file:src/declared.rs".to_string(),
                ],
                acceptance: vec!["completion_ledger.rs:69-78".to_string()],
            },
        );

        assert_eq!(
            harvest_contract_path_intents(&node),
            vec![
                "completion_ledger.rs".to_string(),
                "queue.rs".to_string(),
                "runtime.rs".to_string(),
            ]
        );
    }

    #[test]
    fn knowledge_resolution_keeps_success_when_another_intent_fails() {
        let graph = TaskGraph::new(
            vec![task(
                "T1",
                NodeContract {
                    inputs: vec!["file:src/good.rs".to_string()],
                    outputs: vec!["file:src/missing.rs".to_string()],
                    acceptance: Vec::new(),
                },
            )],
            Vec::new(),
        );
        let coverage = TouchCoverageReport {
            available: true,
            artifact_languages: BTreeSet::from(["rust".to_string()]),
            touches: BTreeMap::new(),
            unresolved_task_ids: vec!["T1".to_string()],
        };
        let resolver = CandidateResolver {
            coverage: coverage.clone(),
            candidates: BTreeSet::from(["src/good.rs".to_string()]),
        };

        let mut config = enabled_config();
        config.parent_directory = false;
        let knowledge = build_knowledge_touch_coverage(
            &graph,
            &resolver,
            &coverage,
            None,
            config,
        );

        assert_eq!(
            knowledge.knowledge_attachment_touches,
            BTreeMap::from([(
                "T1".to_string(),
                BTreeSet::from(["src/good.rs".to_string()]),
            )])
        );
        assert_eq!(knowledge.resolution_omissions.len(), 1);
        assert_eq!(
            knowledge.resolution_omissions[0].detail,
            "knowledge path resolution found no tracked path"
        );
        assert_eq!(
            knowledge.resolution_omissions[0].examples,
            vec!["T1: src/missing.rs"]
        );
    }

    #[test]
    fn new_file_intent_resolves_to_bounded_non_root_parent() {
        let candidates = BTreeSet::from([
            "src/new/existing.rs".to_string(),
            "src/other.rs".to_string(),
        ]);

        assert_eq!(
            resolve_path_intent("src/new/generated.rs", &candidates, true),
            Ok(PathResolution {
                path: "src/new".to_string(),
                match_type: PathMatchType::ParentDirectory,
            })
        );
        assert_eq!(
            resolve_path_intent("generated.rs", &candidates, true),
            Err(PathResolutionFailure::NoMatch),
            "parent fallback requires a directory component"
        );

        let crowded: BTreeSet<_> = (0..=MAX_KNOWLEDGE_TOUCHES_PER_TASK)
            .map(|index| format!("src/crowded/file-{index}.rs"))
            .collect();
        assert_eq!(
            resolve_path_intent("src/crowded/generated.rs", &crowded, true),
            Err(PathResolutionFailure::NoMatch),
            "parent fallback must reject more than 256 descendants"
        );
    }

    #[test]
    fn inferred_scope_records_match_provenance_and_stale_omissions() {
        let root = knowledge_project(
            "## Exact\nUse `src/exact.rs`.\n\n## Basename\nUse `runtime.rs`.\n\n## Suffix\nUse `work_graph/suffix.rs`.\n\n## Stale\nUse `missing.rs`.\n",
        );
        let mut graph = TaskGraph::new(
            vec![
                task("T1", NodeContract::default()),
                task("T2", NodeContract::default()),
                task("T3", NodeContract::default()),
            ],
            Vec::new(),
        );
        let touches = BTreeMap::from([
            ("T1".to_string(), BTreeSet::from(["src/exact.rs".to_string()])),
            (
                "T2".to_string(),
                BTreeSet::from(["src/orchestrator/runtime.rs".to_string()]),
            ),
            (
                "T3".to_string(),
                BTreeSet::from(["src/work_graph/suffix.rs".to_string()]),
            ),
        ]);
        let coverage = TouchCoverageReport {
            available: true,
            artifact_languages: BTreeSet::new(),
            touches: touches.clone(),
            unresolved_task_ids: Vec::new(),
        };
        let candidates = touches.values().flatten().cloned().collect();
        let knowledge = KnowledgeTouchCoverage {
            declared_touches: touches.clone(),
            knowledge_attachment_touches: touches,
            provenance_by_task: BTreeMap::new(),
            resolution_omissions: Vec::new(),
        };

        let report = derive_project_context_from_knowledge(
            &mut graph,
            root.path(),
            None,
            &coverage,
            &knowledge,
            Some(&candidates),
            enabled_config(),
        );

        assert_eq!(report.knowledge_edge_count, 3);
        let rationales: BTreeSet<_> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Informs)
            .filter_map(|edge| edge.rationale.clone())
            .collect();
        assert!(rationales.iter().any(|value| value.contains("inferred-scope:exact")));
        assert!(rationales
            .iter()
            .any(|value| value.contains("inferred-scope:unique-basename")));
        assert!(rationales
            .iter()
            .any(|value| value.contains("inferred-scope:path-suffix")));
        let stale = graph
            .omissions
            .iter()
            .find(|omission| {
                omission.reason == WorkGraphOmissionReason::ResolutionIncomplete
                    && omission.detail == "inferred knowledge scope found no tracked path"
            })
            .expect("stale inferred scope omission");
        assert_eq!(stale.count, 1);
        assert_eq!(stale.examples.len(), 1);
        assert!(report
            .gotchas
            .iter()
            .any(|gotcha| gotcha.summary.starts_with("Stale:")));
        assert!(report.gotchas.iter().any(|gotcha| gotcha
            .scope
            .contains(&"src/orchestrator/runtime.rs".to_string())));
    }

    #[test]
    fn expanded_hub_preserves_the_base_declared_edge() {
        let root = knowledge_project(
            "## Shared rule\n**Scope**: `src/shared.rs`\nKeep shared code consistent.\n",
        );
        let mut graph = TaskGraph::new(
            (1..=4)
                .map(|index| task(&format!("T{index}"), NodeContract::default()))
                .collect(),
            Vec::new(),
        );
        let declared = TouchCoverageReport {
            available: true,
            artifact_languages: BTreeSet::new(),
            touches: BTreeMap::from([(
                "T1".to_string(),
                BTreeSet::from(["src/shared.rs".to_string()]),
            )]),
            unresolved_task_ids: Vec::new(),
        };
        let expanded = BTreeMap::from([
            ("T1".to_string(), BTreeSet::from(["src/shared.rs".to_string()])),
            ("T2".to_string(), BTreeSet::from(["src/shared.rs".to_string()])),
            ("T3".to_string(), BTreeSet::from(["src/shared.rs".to_string()])),
        ]);
        let knowledge = KnowledgeTouchCoverage {
            declared_touches: declared.touches.clone(),
            knowledge_attachment_touches: expanded,
            provenance_by_task: BTreeMap::new(),
            resolution_omissions: Vec::new(),
        };

        let report = derive_project_context_from_knowledge(
            &mut graph,
            root.path(),
            None,
            &declared,
            &knowledge,
            Some(&BTreeSet::from(["src/shared.rs".to_string()])),
            enabled_config(),
        );

        let linked: Vec<_> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Informs)
            .map(|edge| edge.target.clone())
            .collect();
        assert_eq!(linked, vec!["T1"]);
        assert_eq!(report.knowledge_edge_count, 1);
        assert_eq!(report.hub_lints.len(), 1);
        assert_eq!(report.hub_lints[0].linked_task_ids, vec!["T1", "T2", "T3"]);
        assert!(report.hub_lints[0].detail.contains("base declared-scope edges were preserved"));
    }

    #[test]
    fn global_cross_reference_uses_contained_wiki_metadata_and_falls_back() {
        let root = knowledge_project(
            "## Local heading\n**Scope**: `src/main.rs`\n-> global: patterns/guide.md\n-> global: practices/heading.md\n",
        );
        let wiki = TempDir::new().expect("wiki tempdir");
        fs::create_dir_all(wiki.path().join("patterns")).expect("wiki category");
        fs::create_dir_all(wiki.path().join("practices")).expect("wiki category");
        fs::write(
            wiki.path().join("patterns/guide.md"),
            "---\ntitle: Reliable planning\ndescription: Keep evidence attached to decisions.\n---\n# Ignored heading\n",
        )
        .expect("wiki page");
        fs::write(
            wiki.path().join("practices/heading.md"),
            "---\ndescription: Use the first heading when title is absent.\n---\n# Heading fallback\n",
        )
        .expect("wiki page");
        let coverage = TouchCoverageReport {
            available: true,
            artifact_languages: BTreeSet::new(),
            touches: BTreeMap::new(),
            unresolved_task_ids: Vec::new(),
        };
        let knowledge = KnowledgeTouchCoverage {
            declared_touches: BTreeMap::new(),
            knowledge_attachment_touches: BTreeMap::new(),
            provenance_by_task: BTreeMap::new(),
            resolution_omissions: Vec::new(),
        };
        let mut graph = TaskGraph::new(Vec::new(), Vec::new());
        let report = derive_project_context_from_knowledge(
            &mut graph,
            root.path(),
            Some(wiki.path()),
            &coverage,
            &knowledge,
            None,
            enabled_config(),
        );
        assert!(report.gotchas.iter().any(|gotcha| {
            gotcha.source_ref == "global:patterns/guide.md"
                && gotcha.summary
                    == "Reliable planning: Keep evidence attached to decisions."
        }));
        assert!(report.gotchas.iter().any(|gotcha| {
            gotcha.source_ref == "global:practices/heading.md"
                && gotcha.summary
                    == "Heading fallback: Use the first heading when title is absent."
        }));

        let mut fallback_graph = TaskGraph::new(Vec::new(), Vec::new());
        let fallback = derive_project_context_from_knowledge(
            &mut fallback_graph,
            root.path(),
            Some(&root.path().join("missing-wiki")),
            &coverage,
            &knowledge,
            None,
            enabled_config(),
        );
        assert!(fallback.gotchas.iter().any(|gotcha| {
            gotcha.source_ref == "global:patterns/guide.md"
                && gotcha.summary == "Local heading: institutional context"
        }));
    }

    #[test]
    fn knowledge_resolution_omissions_are_aggregated_by_stable_category() {
        let graph = TaskGraph::new(
            vec![task(
                "T1",
                NodeContract {
                    inputs: vec!["file:missing-a.rs".to_string()],
                    outputs: vec!["file:missing-b.rs".to_string()],
                    acceptance: (0..6)
                        .map(|index| format!("file:missing-{index}.rs"))
                        .collect(),
                },
            )],
            Vec::new(),
        );
        let coverage = TouchCoverageReport {
            available: true,
            artifact_languages: BTreeSet::new(),
            touches: BTreeMap::new(),
            unresolved_task_ids: Vec::new(),
        };
        let resolver = CandidateResolver {
            coverage: coverage.clone(),
            candidates: BTreeSet::new(),
        };
        let knowledge = build_knowledge_touch_coverage(
            &graph,
            &resolver,
            &coverage,
            None,
            enabled_config(),
        );
        assert_eq!(knowledge.resolution_omissions.len(), 1);
        assert_eq!(knowledge.resolution_omissions[0].count, 8);
        assert_eq!(knowledge.resolution_omissions[0].examples.len(), 5);
        assert_eq!(
            knowledge.resolution_omissions[0].detail,
            "knowledge path resolution found no tracked path"
        );
    }

    fn enabled_config() -> KnowledgeAttachmentConfig {
        KnowledgeAttachmentConfig {
            per_intent: true,
            contract_harvest: true,
            exact: true,
            unique_basename: true,
            path_suffix: true,
            parent_directory: true,
            ambiguous: false,
            resolution_omissions: true,
            inferred_scope: true,
            file_inventory_fallback: true,
        }
    }

    fn task(id: &str, contract: NodeContract) -> WorkNode {
        WorkNode::new(
            id,
            NodeKind::Task,
            format!("Task {id}"),
            contract,
            BindingRef::Role("backend".to_string()),
            NodeStatus::Ready,
        )
    }

    fn knowledge_project(project_dna: &str) -> TempDir {
        let root = TempDir::new().expect("project tempdir");
        let ai_docs = root.path().join(".ai-docs");
        fs::create_dir_all(&ai_docs).expect("knowledge dir");
        fs::write(ai_docs.join("project-dna.md"), project_dna).expect("project DNA");
        fs::write(ai_docs.join("bug-patterns.md"), "").expect("bug patterns");
        fs::write(ai_docs.join("learnings.jsonl"), "").expect("learnings");
        fs::write(
            ai_docs.join("curation-state.json"),
            "{\"last_curated_line\":0}",
        )
        .expect("curation state");
        root
    }
}
