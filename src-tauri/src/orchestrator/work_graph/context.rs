//! Strictly-derived project-knowledge context for issue #218.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

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
const MAX_OMISSION_EXAMPLES: usize = 5;

/// Future codegraph integration seam implemented by WS-8 (#215).
///
/// `Ok(None)` means codegraph/touches data was unavailable. `Ok(Some(empty))`
/// means resolution ran successfully and found no touched modules.
pub trait TouchesResolver {
    fn resolve_touches(
        &self,
        graph: &TaskGraph,
    ) -> Result<Option<BTreeMap<TaskId, BTreeSet<String>>>, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoTouchesResolver;

impl TouchesResolver for NoTouchesResolver {
    fn resolve_touches(
        &self,
        _graph: &TaskGraph,
    ) -> Result<Option<BTreeMap<TaskId, BTreeSet<String>>>, String> {
        Ok(None)
    }
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
    let load = load_project_knowledge(project_path);
    graph.omissions.extend(load.omissions);
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

    let touches = match resolver.resolve_touches(graph) {
        Ok(Some(touches)) => touches,
        Ok(None) => {
            graph.omissions.push(WorkGraphOmission::new(
                WorkGraphOmissionReason::CodegraphUnavailable,
                1,
                vec!["touches-resolver".to_string()],
            ));
            append_context_nodes(graph, &load.gotchas);
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
            append_context_nodes(graph, &load.gotchas);
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

    append_context_nodes(graph, &load.gotchas);
    let all_task_ids: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Task)
        .map(|node| node.id.clone())
        .collect();
    // `Some(empty)` is the interface's successful repo-wide "nothing
    // touched" result. Once any task facts are present, omitted task IDs are
    // instead partial resolution and must be reported.
    let missing_task_ids: Vec<_> = if touches.is_empty() {
        Vec::new()
    } else {
        all_task_ids
            .iter()
            .filter(|task_id| !touches.contains_key(*task_id))
            .cloned()
            .collect()
    };
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
    let mut candidates: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
    for gotcha in &load.gotchas {
        let context_id = context_node_id(&gotcha.id);
        for task_id in &task_ids {
            let Some(task_touches) = touches.get(task_id) else {
                continue;
            };
            if scope_intersects(&gotcha.scope, task_touches) {
                candidates
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
        let mut linked = candidates.remove(&context_id).unwrap_or_default();
        // `*` declares repo-wide applicability. Even partial or successfully
        // empty touch facts cannot make that declaration task-specific, so
        // lint it against the complete plan while still adding no edges.
        if gotcha.scope.iter().any(|scope| scope == "*")
            && task_ids.len() >= ANTI_HUB_MIN_TASKS
        {
            linked = task_ids.clone();
        }
        let fraction = if task_ids.is_empty() {
            0.0
        } else {
            linked.len() as f64 / task_ids.len() as f64
        };
        if task_ids.len() >= ANTI_HUB_MIN_TASKS && fraction >= ANTI_HUB_TASK_FRACTION {
            hub_lints.push(ContextHubLint {
                context_node_id: context_id,
                linked_task_ids: linked,
                task_fraction: fraction,
                threshold: ANTI_HUB_TASK_FRACTION,
                detail: "context applies to a high fraction of tasks; move standing guidance to the role prompt or narrow its scope".to_string(),
            });
            continue;
        }
        for task_id in linked {
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

/// A real adapter for the WS-7 `GotchaAttachmentProvider` seam. The provider
/// derives scoped graph context on demand and returns only non-hub task
/// attachments. Missing project knowledge remains `Ok(None)`.
pub struct ProjectKnowledgeGotchaProvider<R> {
    pub graph: TaskGraph,
    pub resolver: R,
}

impl<R: TouchesResolver> GotchaAttachmentProvider for ProjectKnowledgeGotchaProvider<R> {
    fn gotchas(&self, project_path: &Path) -> Result<Option<Vec<GotchaAttachment>>, String> {
        let mut graph = self.graph.clone();
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
    fingerprints: Vec<KnowledgeSourceFingerprint>,
    omissions: Vec<WorkGraphOmission>,
    available: bool,
}

fn load_project_knowledge(project_path: &Path) -> KnowledgeLoad {
    let ai_docs = project_path.join(".ai-docs");
    if !ai_docs.is_dir() {
        return KnowledgeLoad {
            gotchas: Vec::new(),
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
                    parse_markdown(filename, &content, &source_hash, &mut gotchas);
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
    gotchas: &mut Vec<DerivedGotcha>,
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
                gotchas,
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
        gotchas,
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
    gotchas: &mut Vec<DerivedGotcha>,
) {
    if heading.is_empty()
        || matches!(heading.to_ascii_lowercase().as_str(), "template" | "bugs")
        || body.iter().all(|line| line.trim().is_empty())
    {
        return;
    }
    let scope = markdown_scope(body);
    let summary = markdown_summary(heading, body);
    if summary.is_empty() {
        return;
    }
    let source_ref = format!(".ai-docs/{filename}#L{start_line}");
    gotchas.push(DerivedGotcha {
        id: stable_hash(format!("{source_ref}:{heading}").as_bytes()),
        scope: scope.clone(),
        summary,
        fingerprint_ref: format!(".ai-docs/{filename}"),
        source_ref,
        source_hash: source_hash.to_string(),
    });

    for line in body {
        if let Some(global_ref) = global_cross_ref(line) {
            let source_ref = format!("global:{global_ref}");
            gotchas.push(DerivedGotcha {
                id: stable_hash(format!("{source_ref}:{heading}").as_bytes()),
                scope: scope.clone(),
                summary: bound_summary(&format!("{heading}: institutional context")),
                fingerprint_ref: format!(".ai-docs/{filename}"),
                source_ref,
                source_hash: source_hash.to_string(),
            });
        }
    }
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

fn append_context_nodes(graph: &mut TaskGraph, gotchas: &[DerivedGotcha]) {
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
