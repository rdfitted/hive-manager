//! Deterministic role, task, and inherited-context composition.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::boundary::{includes_artifact_context, includes_spawner_conversation};
use super::definitions::{MAX_KNOWLEDGE_POINTER_CHARS, MAX_KNOWLEDGE_SUMMARY_CHARS};
use super::schema::{
    ContextBoundary, KnowledgeRef, KnowledgeSource, RoleDefinition, RoleLens,
};
use crate::orchestrator::work_graph::context::{
    ANTI_HUB_MIN_TASKS, ANTI_HUB_TASK_FRACTION,
};
use crate::orchestrator::work_graph::{EdgeKind, EdgeProvenance, TaskGraph};

pub const ACK_SAMPLE_RATE: f64 = 0.33;
pub const ACK_QUESTION_VERSION: u32 = 1;
pub const ACK_MISS_SAMPLE_MAX: usize = 1;
pub const ACK_INSTRUCTION: &str = "When you send your completed heartbeat, include \"knowledge_ack\":[\"k2\",...] listing the references that were relevant to your work, or \"knowledge_ack\":[] if none were relevant.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextOrigin {
    Role,
    Task,
    RoleAndTask,
    Conversation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextDropReason {
    RoleBudgetExceeded,
    TaskBudgetExceeded,
    ConversationBudgetExceeded,
    BoundaryExcluded,
    SourceBoundsExceeded,
}

impl ContextDropReason {
    fn description(self) -> &'static str {
        match self {
            Self::RoleBudgetExceeded => "role context budget exceeded",
            Self::TaskBudgetExceeded => "task context budget exceeded",
            Self::ConversationBudgetExceeded => "conversation context budget exceeded",
            Self::BoundaryExcluded => "role context boundary excluded this source",
            Self::SourceBoundsExceeded => "source pointer or summary exceeded declared bounds",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposedKnowledgeRef {
    pub reference: KnowledgeRef,
    pub origin: ContextOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextConflict {
    pub source: KnowledgeSource,
    pub pointer: String,
    pub role_guidance: String,
    pub task_guidance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DroppedContext {
    pub pointer: String,
    pub origin: ContextOrigin,
    pub reason: ContextDropReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudget {
    pub role_chars: usize,
    pub task_chars: usize,
    pub conversation_chars: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            role_chars: 4_096,
            task_chars: 4_096,
            conversation_chars: 4_096,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawner_conversation: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnContext {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub task_scope: Vec<KnowledgeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_summary: Option<String>,
    #[serde(default)]
    pub conversation: ConversationContext,
    #[serde(default)]
    pub budget: ContextBudget,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposedContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens: Option<RoleLens>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_goals: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<ComposedKnowledgeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawner_conversation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<ContextConflict>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<DroppedContext>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionResult {
    pub context: ComposedContext,
    pub remaining: ContextBudget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoleKnowledgeHubLint {
    pub source: KnowledgeSource,
    pub role_id: String,
    pub pointer: String,
    pub affected_role_ids: Vec<String>,
    pub role_fraction: f64,
    pub threshold: f64,
    pub detail: String,
}

#[derive(Debug, Clone)]
struct PendingKnowledge {
    reference: KnowledgeRef,
    origin: ContextOrigin,
    role_summary: Option<String>,
    task_summary: Option<String>,
}

/// Merge role and task scope once, preserving the role's framing while making
/// disagreement and every budget exclusion visible.
pub fn compose_context(
    role: Option<&RoleDefinition>,
    spawn: &SpawnContext,
) -> ComposedContext {
    compose_context_with_remaining(role, spawn).context
}

/// Compose context while retaining the exact remaining character budgets used
/// for optional post-composition sampling.
pub fn compose_context_with_remaining(
    role: Option<&RoleDefinition>,
    spawn: &SpawnContext,
) -> CompositionResult {
    let mut composed = ComposedContext::default();
    let mut role_remaining = spawn.budget.role_chars;
    let mut task_remaining = spawn.budget.task_chars;
    let mut conversation_remaining = spawn.budget.conversation_chars;

    if let Some(role) = role {
        admit_role_metadata(role, &mut role_remaining, &mut composed);
    }

    if let Some(summary) = nonempty(spawn.task_summary.as_deref()) {
        let cost = text_record_cost("task-summary", summary);
        if take_budget(&mut task_remaining, cost) {
            composed.task_summary = Some(summary.to_string());
        } else {
            composed.dropped.push(DroppedContext {
                pointer: "task-summary".to_string(),
                origin: ContextOrigin::Task,
                reason: ContextDropReason::TaskBudgetExceeded,
            });
        }
    }

    let mut pending = BTreeMap::<String, PendingKnowledge>::new();
    if let Some(role) = role {
        for reference in &role.knowledge_scope {
            insert_knowledge(
                &mut pending,
                reference,
                ContextOrigin::Role,
                &mut composed.dropped,
            );
        }
    }
    for reference in &spawn.task_scope {
        insert_knowledge(
            &mut pending,
            reference,
            ContextOrigin::Task,
            &mut composed.dropped,
        );
    }

    let mut records: Vec<_> = pending.into_values().collect();
    records.sort_by(|left, right| {
        right
            .reference
            .priority
            .cmp(&left.reference.priority)
            .then(origin_rank(left.origin).cmp(&origin_rank(right.origin)))
            .then(knowledge_key(&left.reference).cmp(&knowledge_key(&right.reference)))
    });

    for record in records {
        if let (Some(role_guidance), Some(task_guidance)) =
            (record.role_summary.as_deref(), record.task_summary.as_deref())
        {
            if role_guidance != task_guidance {
                composed.conflicts.push(ContextConflict {
                    source: record.reference.source,
                    pointer: record.reference.pointer.clone(),
                    role_guidance: role_guidance.to_string(),
                    task_guidance: task_guidance.to_string(),
                });
            }
        }

        let cost = knowledge_reference_chars(&record.reference);
        let (remaining, reason) = match record.origin {
            ContextOrigin::Role | ContextOrigin::RoleAndTask => {
                (&mut role_remaining, ContextDropReason::RoleBudgetExceeded)
            }
            ContextOrigin::Task => {
                (&mut task_remaining, ContextDropReason::TaskBudgetExceeded)
            }
            ContextOrigin::Conversation => unreachable!("conversation is not knowledge scope"),
        };
        if take_budget(remaining, cost) {
            composed.knowledge.push(ComposedKnowledgeRef {
                reference: record.reference,
                origin: record.origin,
            });
        } else {
            composed.dropped.push(DroppedContext {
                pointer: record.reference.pointer,
                origin: record.origin,
                reason,
            });
        }
    }

    let boundary = role
        .map(|definition| definition.context_boundary)
        .unwrap_or_else(ContextBoundary::default);
    admit_conversation_context(
        &spawn.conversation,
        boundary,
        &mut conversation_remaining,
        &mut composed,
    );

    composed.conflicts.sort_by(|left, right| {
        knowledge_source_key(left.source)
            .cmp(knowledge_source_key(right.source))
            .then(left.pointer.cmp(&right.pointer))
    });
    composed.dropped.sort_by(|left, right| {
        origin_rank(left.origin)
            .cmp(&origin_rank(right.origin))
            .then(left.pointer.cmp(&right.pointer))
            .then(drop_reason_rank(left.reason).cmp(&drop_reason_rank(right.reason)))
    });
    CompositionResult {
        context: composed,
        remaining: ContextBudget {
            role_chars: role_remaining,
            task_chars: task_remaining,
            conversation_chars: conversation_remaining,
        },
    }
}

/// Convert the authoritative knowledge edges for one explicitly identified
/// work-graph task into spawn context. Missing nodes or IDs produce an empty
/// context; callers must not infer a replacement task from prose.
pub fn spawn_context_from_work_graph_task(
    graph: &TaskGraph,
    plan_task_id: &str,
) -> SpawnContext {
    let plan_task_id = plan_task_id.trim();
    let Some(task_node) = graph.nodes.iter().find(|node| node.id == plan_task_id) else {
        return SpawnContext::default();
    };
    let task_scope = graph
        .edges
        .iter()
        .filter(|edge| {
            edge.target == plan_task_id
                && edge.kind == EdgeKind::Informs
                && edge.provenance == EdgeProvenance::Knowledge
        })
        .filter_map(|edge| graph.nodes.iter().find(|node| node.id == edge.source))
        .filter_map(knowledge_ref_from_work_node)
        .collect();

    SpawnContext {
        task_scope,
        task_summary: Some(bound_context_text(
            &task_node.title,
            MAX_KNOWLEDGE_SUMMARY_CHARS,
        ))
        .filter(|summary| !summary.is_empty()),
        ..SpawnContext::default()
    }
}

/// Collect every bounded knowledge reference represented in a work graph. The
/// caller decides which of these unattached/dropped references are eligible for
/// miss sampling; this function does not infer a task.
pub fn knowledge_refs_from_work_graph(graph: &TaskGraph) -> Vec<KnowledgeRef> {
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == crate::orchestrator::work_graph::NodeKind::Context)
        .filter_map(knowledge_ref_from_work_node)
        .collect()
}

fn knowledge_ref_from_work_node(
    source_node: &crate::orchestrator::work_graph::WorkNode,
) -> Option<KnowledgeRef> {
    let expansion = source_node.expansion.as_ref()?;
    let source_ref = expansion.parameters.get("source_ref")?.trim();
    if source_ref.is_empty() {
        return None;
    }
    let summary = expansion
        .parameters
        .get("summary")
        .map(|summary| bound_context_text(summary, MAX_KNOWLEDGE_SUMMARY_CHARS))
        .filter(|summary| !summary.is_empty());
    let (source, pointer) = if let Some(pointer) = source_ref
        .strip_prefix("institutional:")
        .or_else(|| source_ref.strip_prefix("global:"))
    {
        (KnowledgeSource::Institutional, pointer.trim())
    } else {
        (KnowledgeSource::Project, source_ref)
    };
    let priority = expansion
        .parameters
        .get("priority")
        .and_then(|priority| priority.parse().ok())
        .unwrap_or_default();
    Some(KnowledgeRef {
        source,
        pointer: pointer.to_string(),
        summary,
        priority,
    })
}

/// Render one stable prompt section. An empty input produces zero bytes so
/// legacy spawns retain their exact prompt.
pub fn render_composed_context(context: &ComposedContext) -> String {
    if context == &ComposedContext::default() {
        return String::new();
    }

    let mut rendered = String::from("## Composed Role and Task Context\n\n");
    if let Some(domain) = context.domain.as_deref() {
        rendered.push_str(&format!("Domain: {domain}\n\n"));
    }
    if let Some(lens) = context.lens.as_ref() {
        rendered.push_str(&format!(
            "Lens `{}`: {}\n\n",
            lens.id, lens.question
        ));
    }
    if !context.non_goals.is_empty() {
        rendered.push_str("### Declared Non-Goals\n\n");
        for non_goal in &context.non_goals {
            rendered.push_str(&format!("- {non_goal}\n"));
        }
        rendered.push('\n');
    }
    if !context.knowledge.is_empty() {
        rendered.push_str("### Knowledge References\n\n");
        for item in &context.knowledge {
            let summary = item
                .reference
                .summary
                .as_deref()
                .map(|summary| format!(": {summary}"))
                .unwrap_or_default();
            rendered.push_str(&format!(
                "- [{}] `{}` (priority {}){}\n",
                origin_label(item.origin),
                display_reference(&item.reference),
                item.reference.priority,
                summary
            ));
        }
        rendered.push('\n');
    }
    if let Some(summary) = context.task_summary.as_deref() {
        rendered.push_str(&format!("### Task Summary\n\n{summary}\n\n"));
    }
    if let Some(artifact) = context.artifact_context.as_deref() {
        rendered.push_str(&format!("### Artifact Context\n\n{artifact}\n\n"));
    }
    if let Some(conversation) = context.spawner_conversation.as_deref() {
        rendered.push_str(&format!(
            "### Spawner Conversation\n\n{conversation}\n\n"
        ));
    }
    if !context.conflicts.is_empty() {
        rendered.push_str("### Context Conflicts\n\n");
        for conflict in &context.conflicts {
            rendered.push_str(&format!(
                "- `{}`: role guidance = {:?}; task guidance = {:?}\n",
                display_source_pointer(conflict.source, &conflict.pointer),
                conflict.role_guidance,
                conflict.task_guidance
            ));
        }
        rendered.push('\n');
    }
    if !context.dropped.is_empty() {
        rendered.push_str("### Dropped Context\n\n");
        for dropped in &context.dropped {
            rendered.push_str(&format!(
                "- `{}` [{}]: {}\n",
                dropped.pointer,
                origin_label(dropped.origin),
                dropped.reason.description()
            ));
        }
        rendered.push('\n');
    }
    rendered
}

/// Render acknowledgement tags without changing the legacy renderer used by
/// composition-sidecar and unsampled spawns.
pub fn render_composed_context_with_ack_tags(context: &ComposedContext) -> String {
    if context.knowledge.is_empty() {
        return render_composed_context(context);
    }

    let mut rendered = render_composed_context(context);
    let marker = "### Knowledge References\n\n";
    let Some(mut search_start) = rendered.find(marker).map(|index| index + marker.len()) else {
        return rendered;
    };

    for (index, item) in context.knowledge.iter().enumerate() {
        let summary = item
            .reference
            .summary
            .as_deref()
            .map(|summary| format!(": {summary}"))
            .unwrap_or_default();
        let legacy_line = format!(
            "- [{}] `{}` (priority {}){}\n",
            origin_label(item.origin),
            display_reference(&item.reference),
            item.reference.priority,
            summary
        );
        let tagged_line = format!(
            "- [k{}] [{}] `{}` (priority {}){}\n",
            index + 1,
            origin_label(item.origin),
            display_reference(&item.reference),
            item.reference.priority,
            summary
        );
        let Some(offset) = rendered[search_start..].find(&legacy_line) else {
            return render_composed_context(context);
        };
        let line_start = search_start + offset;
        let line_end = line_start + legacy_line.len();
        rendered.replace_range(line_start..line_end, &tagged_line);
        search_start = line_start + tagged_line.len();
    }
    rendered.insert_str(search_start, &format!("\n{ACK_INSTRUCTION}\n"));
    rendered
}

/// Stable, replayable acknowledgement draw keyed only on the full agent id.
pub fn is_ack_sampled(agent_id: &str) -> bool {
    let namespace = Uuid::from_u128(0x7af0b20a_83df_4bc3_a2b6_138881df6757);
    let draw = Uuid::new_v5(&namespace, agent_id.as_bytes());
    let mut leading = [0_u8; 8];
    leading.copy_from_slice(&draw.as_bytes()[..8]);
    u64::from_be_bytes(leading) % 10_000 < 3_300
}

/// Admit at most one deterministic miss reference after ordinary composition.
/// Candidates are filtered against the exact remaining task budget first, so a
/// miss can never displace a normal kept reference.
pub fn admit_ack_miss_sample(
    context: &mut ComposedContext,
    remaining: &mut ContextBudget,
    candidates: &[KnowledgeRef],
    agent_id: &str,
) -> Option<usize> {
    if ACK_MISS_SAMPLE_MAX == 0 {
        return None;
    }

    let kept = context
        .knowledge
        .iter()
        .map(|item| knowledge_key(&item.reference))
        .collect::<BTreeSet<_>>();
    let mut eligible = BTreeMap::<String, KnowledgeRef>::new();
    for candidate in candidates {
        let key = knowledge_key(candidate);
        if !within_source_bounds(candidate)
            || kept.contains(&key)
            || knowledge_reference_chars(candidate) > remaining.task_chars
        {
            continue;
        }
        eligible.entry(key).or_insert_with(|| candidate.clone());
    }
    if eligible.is_empty() {
        return None;
    }

    let namespace = Uuid::from_u128(0x0a799f91_70df_4613_8740_1a0ab670c773);
    let draw = Uuid::new_v5(&namespace, agent_id.as_bytes());
    let mut leading = [0_u8; 8];
    leading.copy_from_slice(&draw.as_bytes()[..8]);
    let selected = u64::from_be_bytes(leading) as usize % eligible.len();
    let reference = eligible.into_values().nth(selected)?;
    remaining.task_chars -= knowledge_reference_chars(&reference);
    context.knowledge.push(ComposedKnowledgeRef {
        reference,
        origin: ContextOrigin::Task,
    });
    Some(context.knowledge.len() - 1)
}

/// Fraction-based anti-hub lint for declarations that name an entire knowledge
/// tree. A specific page may legitimately be shared by every role and is not a
/// hub; a wildcard/root scope is treated as affecting the complete definition
/// universe and checked against the existing work-graph thresholds.
pub fn lint_role_knowledge_hubs(definitions: &[RoleDefinition]) -> Vec<RoleKnowledgeHubLint> {
    let mut role_ids: Vec<_> = definitions
        .iter()
        .map(|definition| definition.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    role_ids.sort();
    if role_ids.len() < ANTI_HUB_MIN_TASKS {
        return Vec::new();
    }
    // A whole-tree declaration reaches the complete definition universe, not
    // only the role that authored the declaration.
    let affected_role_ids = role_ids.clone();
    let fraction = affected_role_ids.len() as f64 / role_ids.len() as f64;
    if fraction < ANTI_HUB_TASK_FRACTION {
        return Vec::new();
    }

    let mut lints = Vec::new();
    for definition in definitions {
        for reference in &definition.knowledge_scope {
            let pointer = normalize_pointer(&reference.pointer);
            if is_whole_tree_scope(&pointer) {
                lints.push(RoleKnowledgeHubLint {
                    source: reference.source,
                    role_id: definition.id.clone(),
                    pointer,
                    affected_role_ids: affected_role_ids.clone(),
                    role_fraction: fraction,
                    threshold: ANTI_HUB_TASK_FRACTION,
                    detail: "role knowledge scope covers a whole tree; replace it with discriminating references".to_string(),
                });
            }
        }
    }
    lints.sort_by(|left, right| {
        knowledge_source_key(left.source)
            .cmp(knowledge_source_key(right.source))
            .then(left.role_id.cmp(&right.role_id))
            .then(left.pointer.cmp(&right.pointer))
    });
    lints
}

fn admit_role_metadata(
    role: &RoleDefinition,
    remaining: &mut usize,
    composed: &mut ComposedContext,
) {
    if let Some(domain) = nonempty(role.domain.as_deref()) {
        if take_budget(remaining, text_record_cost("role-domain", domain)) {
            composed.domain = Some(domain.to_string());
        } else {
            composed.dropped.push(DroppedContext {
                pointer: "role-domain".to_string(),
                origin: ContextOrigin::Role,
                reason: ContextDropReason::RoleBudgetExceeded,
            });
        }
    }
    if let Some(lens) = role.lens.as_ref() {
        let cost = text_record_cost(&lens.id, &lens.question);
        if take_budget(remaining, cost) {
            composed.lens = Some(lens.clone());
        } else {
            composed.dropped.push(DroppedContext {
                pointer: format!("role-lens:{}", lens.id),
                origin: ContextOrigin::Role,
                reason: ContextDropReason::RoleBudgetExceeded,
            });
        }
    }
    for (index, non_goal) in role.non_goals.iter().enumerate() {
        let pointer = format!("role-non-goal-{}", index + 1);
        if take_budget(remaining, text_record_cost(&pointer, non_goal)) {
            composed.non_goals.push(non_goal.clone());
        } else {
            composed.dropped.push(DroppedContext {
                pointer,
                origin: ContextOrigin::Role,
                reason: ContextDropReason::RoleBudgetExceeded,
            });
        }
    }
}

fn insert_knowledge(
    pending: &mut BTreeMap<String, PendingKnowledge>,
    reference: &KnowledgeRef,
    origin: ContextOrigin,
    dropped: &mut Vec<DroppedContext>,
) {
    if !within_source_bounds(reference) {
        dropped.push(DroppedContext {
            pointer: bounded_pointer_label(&reference.pointer),
            origin,
            reason: ContextDropReason::SourceBoundsExceeded,
        });
        return;
    }
    let mut normalized = reference.clone();
    normalized.pointer = normalize_pointer(&reference.pointer);
    normalized.summary = nonempty(reference.summary.as_deref()).map(ToString::to_string);
    let key = knowledge_key(&normalized);
    if let Some(existing) = pending.get_mut(&key) {
        existing.reference.priority = existing.reference.priority.max(normalized.priority);
        existing.origin = merge_origins(existing.origin, origin);
        match origin {
            ContextOrigin::Role => {
                existing.role_summary = normalized.summary.clone();
                existing.reference.summary = normalized.summary.clone().or_else(|| {
                    existing.task_summary.clone()
                });
            }
            ContextOrigin::Task => {
                existing.task_summary = normalized.summary.clone();
                if existing.role_summary.is_none() {
                    existing.reference.summary = normalized.summary.clone();
                }
            }
            ContextOrigin::RoleAndTask | ContextOrigin::Conversation => {}
        }
        return;
    }

    pending.insert(
        key,
        PendingKnowledge {
            reference: normalized.clone(),
            origin,
            role_summary: if origin == ContextOrigin::Role {
                normalized.summary.clone()
            } else {
                None
            },
            task_summary: if origin == ContextOrigin::Task {
                normalized.summary.clone()
            } else {
                None
            },
        },
    );
}

fn admit_conversation_context(
    conversation: &ConversationContext,
    boundary: ContextBoundary,
    remaining: &mut usize,
    composed: &mut ComposedContext,
) {
    if let Some(artifact) = nonempty(conversation.artifact_summary.as_deref()) {
        if !includes_artifact_context(boundary) {
            composed.dropped.push(DroppedContext {
                pointer: "artifact-context".to_string(),
                origin: ContextOrigin::Conversation,
                reason: ContextDropReason::BoundaryExcluded,
            });
        } else if take_budget(remaining, text_record_cost("artifact-context", artifact)) {
            composed.artifact_context = Some(artifact.to_string());
        } else {
            composed.dropped.push(DroppedContext {
                pointer: "artifact-context".to_string(),
                origin: ContextOrigin::Conversation,
                reason: ContextDropReason::ConversationBudgetExceeded,
            });
        }
    }
    if let Some(spawner) = nonempty(conversation.spawner_conversation.as_deref()) {
        if !includes_spawner_conversation(boundary) {
            composed.dropped.push(DroppedContext {
                pointer: "spawner-conversation".to_string(),
                origin: ContextOrigin::Conversation,
                reason: ContextDropReason::BoundaryExcluded,
            });
        } else if take_budget(
            remaining,
            text_record_cost("spawner-conversation", spawner),
        ) {
            composed.spawner_conversation = Some(spawner.to_string());
        } else {
            composed.dropped.push(DroppedContext {
                pointer: "spawner-conversation".to_string(),
                origin: ContextOrigin::Conversation,
                reason: ContextDropReason::ConversationBudgetExceeded,
            });
        }
    }
}

fn within_source_bounds(reference: &KnowledgeRef) -> bool {
    let pointer_len = reference.pointer.trim().chars().count();
    pointer_len > 0
        && pointer_len <= MAX_KNOWLEDGE_POINTER_CHARS
        && reference
            .summary
            .as_deref()
            .map_or(true, |summary| {
                summary.chars().count() <= MAX_KNOWLEDGE_SUMMARY_CHARS
            })
}

fn knowledge_key(reference: &KnowledgeRef) -> String {
    format!(
        "{}:{}",
        knowledge_source_key(reference.source),
        normalize_pointer(&reference.pointer)
    )
}

fn knowledge_source_key(source: KnowledgeSource) -> &'static str {
    match source {
        KnowledgeSource::Institutional => "institutional",
        KnowledgeSource::Project => "project",
    }
}

fn display_reference(reference: &KnowledgeRef) -> String {
    display_source_pointer(reference.source, &reference.pointer)
}

fn display_source_pointer(source: KnowledgeSource, pointer: &str) -> String {
    format!("{}:{}", knowledge_source_key(source), pointer)
}

fn normalize_pointer(pointer: &str) -> String {
    let mut normalized = pointer.trim().replace('\\', "/");
    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }
    normalized.trim_start_matches("./").to_string()
}

fn is_whole_tree_scope(pointer: &str) -> bool {
    let pointer = pointer.to_ascii_lowercase();
    matches!(pointer.as_str(), "*" | "." | "/" | "wiki" | "global")
        || pointer.ends_with("/*")
        || pointer.ends_with("/**")
}

fn merge_origins(left: ContextOrigin, right: ContextOrigin) -> ContextOrigin {
    match (left, right) {
        (ContextOrigin::Role, ContextOrigin::Task)
        | (ContextOrigin::Task, ContextOrigin::Role)
        | (ContextOrigin::RoleAndTask, ContextOrigin::Role | ContextOrigin::Task)
        | (ContextOrigin::Role | ContextOrigin::Task, ContextOrigin::RoleAndTask) => {
            ContextOrigin::RoleAndTask
        }
        _ => left,
    }
}

fn origin_rank(origin: ContextOrigin) -> u8 {
    match origin {
        ContextOrigin::RoleAndTask => 0,
        ContextOrigin::Role => 1,
        ContextOrigin::Task => 2,
        ContextOrigin::Conversation => 3,
    }
}

fn origin_label(origin: ContextOrigin) -> &'static str {
    match origin {
        ContextOrigin::Role => "role",
        ContextOrigin::Task => "task",
        ContextOrigin::RoleAndTask => "role+task",
        ContextOrigin::Conversation => "conversation",
    }
}

fn drop_reason_rank(reason: ContextDropReason) -> u8 {
    match reason {
        ContextDropReason::RoleBudgetExceeded => 0,
        ContextDropReason::TaskBudgetExceeded => 1,
        ContextDropReason::ConversationBudgetExceeded => 2,
        ContextDropReason::BoundaryExcluded => 3,
        ContextDropReason::SourceBoundsExceeded => 4,
    }
}

pub fn knowledge_reference_chars(reference: &KnowledgeRef) -> usize {
    text_record_cost(
        &display_reference(reference),
        reference.summary.as_deref().unwrap_or_default(),
    )
}

fn text_record_cost(pointer: &str, summary: &str) -> usize {
    pointer.chars().count() + summary.chars().count() + 32
}

fn take_budget(remaining: &mut usize, cost: usize) -> bool {
    if cost > *remaining {
        return false;
    }
    *remaining -= cost;
    true
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn bound_context_text(value: &str, max_chars: usize) -> String {
    value.trim().chars().take(max_chars).collect()
}

fn bounded_pointer_label(pointer: &str) -> String {
    let bounded = bound_context_text(pointer, MAX_KNOWLEDGE_POINTER_CHARS);
    if bounded.is_empty() {
        "<empty-pointer>".to_string()
    } else {
        bounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(pointer: &str) -> KnowledgeRef {
        KnowledgeRef {
            source: KnowledgeSource::Project,
            pointer: pointer.to_string(),
            summary: Some(format!("Guidance for {pointer}")),
            priority: 80,
        }
    }

    #[test]
    fn sampled_renderer_tags_each_reference_and_adds_one_relevance_instruction() {
        let mut multiline = reference("second.md");
        multiline.summary = Some("First line\n\nSecond line".to_string());
        let context = ComposedContext {
            knowledge: vec![
                ComposedKnowledgeRef {
                    reference: reference("first.md"),
                    origin: ContextOrigin::Task,
                },
                ComposedKnowledgeRef {
                    reference: multiline,
                    origin: ContextOrigin::RoleAndTask,
                },
            ],
            task_summary: Some("Task details remain after the tagged section".to_string()),
            ..ComposedContext::default()
        };

        let rendered = render_composed_context_with_ack_tags(&context);
        assert!(rendered.contains("- [k1] [task] `project:first.md`"));
        assert!(rendered.contains("- [k2] [role+task] `project:second.md`"));
        assert!(rendered.contains("First line\n\nSecond line"));
        assert!(rendered.contains("### Task Summary\n\nTask details remain"));
        assert_eq!(rendered.matches("knowledge_ack").count(), 2);
        assert_eq!(rendered.matches(ACK_INSTRUCTION).count(), 1);
        assert!(!ACK_INSTRUCTION.contains("caus"));
        assert!(ACK_INSTRUCTION.contains("relevant"));
    }

    #[test]
    fn acknowledgement_sampling_is_stable_across_calls() {
        let sampled = (0..1000)
            .map(|index| format!("stable-agent-{index}"))
            .find(|id| is_ack_sampled(id))
            .expect("sampled fixture id");
        let unsampled = (0..1000)
            .map(|index| format!("stable-agent-{index}"))
            .find(|id| !is_ack_sampled(id))
            .expect("unsampled fixture id");

        for _ in 0..10 {
            assert!(is_ack_sampled(&sampled));
            assert!(!is_ack_sampled(&unsampled));
        }
    }

    #[test]
    fn miss_sample_uses_only_leftover_budget_without_displacing_kept_references() {
        let kept = reference("kept.md");
        let miss = reference("miss.md");
        let kept_cost = knowledge_reference_chars(&kept);
        let miss_cost = knowledge_reference_chars(&miss);
        let spawn = SpawnContext {
            task_scope: vec![kept.clone()],
            budget: ContextBudget {
                role_chars: 0,
                task_chars: kept_cost + miss_cost,
                conversation_chars: 0,
            },
            ..SpawnContext::default()
        };
        let mut result = compose_context_with_remaining(None, &spawn);
        let ordinary = result.context.knowledge.clone();

        let admitted = admit_ack_miss_sample(
            &mut result.context,
            &mut result.remaining,
            &[kept, miss.clone()],
            "miss-sample-agent",
        );
        assert_eq!(admitted, Some(1));
        assert_eq!(&result.context.knowledge[..1], ordinary.as_slice());
        assert_eq!(result.context.knowledge[1].reference, miss);
        assert_eq!(result.remaining.task_chars, 0);

        let tight_spawn = SpawnContext {
            task_scope: vec![reference("kept.md")],
            budget: ContextBudget {
                role_chars: 0,
                task_chars: kept_cost + miss_cost - 1,
                conversation_chars: 0,
            },
            ..SpawnContext::default()
        };
        let mut tight = compose_context_with_remaining(None, &tight_spawn);
        let before = tight.context.clone();
        assert_eq!(
            admit_ack_miss_sample(
                &mut tight.context,
                &mut tight.remaining,
                &[reference("miss.md")],
                "miss-sample-agent",
            ),
            None
        );
        assert_eq!(tight.context, before);
    }
}
