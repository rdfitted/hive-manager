//! Two-tier role-definition resolution.
//!
//! Institutional definitions are optional configured inputs. The checked-in
//! files are embedded fallbacks so installed applications do not depend on a
//! source checkout, while project overrides are always loaded relative to the
//! session's project path.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::schema::{
    AuthorityScope, ContextBoundary, KnowledgeRef, RoleDefinition, RoleLens, SignalClass,
};
use crate::cli::CliBehavior;

pub const INSTITUTIONAL_ROLE_DIR: &str = "roles";
pub const PROJECT_ROLE_DIR: &str = ".ai-docs/roles";
pub const MAX_KNOWLEDGE_POINTER_CHARS: usize = 256;
pub const MAX_KNOWLEDGE_SUMMARY_CHARS: usize = 240;

const EMBEDDED_ROLE_DEFINITIONS: &[(&str, &str)] = &[
    ("backend", include_str!("../../../../roles/backend.md")),
    ("frontend", include_str!("../../../../roles/frontend.md")),
    ("coherence", include_str!("../../../../roles/coherence.md")),
    ("simplify", include_str!("../../../../roles/simplify.md")),
    ("reviewer", include_str!("../../../../roles/reviewer.md")),
    ("reviewer-quick", include_str!("../../../../roles/reviewer-quick.md")),
    ("resolver", include_str!("../../../../roles/resolver.md")),
    ("tester", include_str!("../../../../roles/tester.md")),
    ("code-quality", include_str!("../../../../roles/code-quality.md")),
    ("researcher", include_str!("../../../../roles/researcher.md")),
    ("general", include_str!("../../../../roles/general.md")),
    (
        "master-planner",
        include_str!("../../../../roles/master-planner.md"),
    ),
    ("queen", include_str!("../../../../roles/queen.md")),
    ("evaluator", include_str!("../../../../roles/evaluator.md")),
    ("prince", include_str!("../../../../roles/prince.md")),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleDefinitionSource {
    Institutional,
    EmbeddedDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleResolutionIssueKind {
    InstitutionalUnavailable,
    ProjectKnowledgeUnavailable,
    DefinitionNotFound,
    SourceUnreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleResolutionIssue {
    pub kind: RoleResolutionIssueKind,
    pub source_ref: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedRoleDefinition {
    pub requested_id: String,
    pub definition: Option<RoleDefinition>,
    pub base_source: Option<RoleDefinitionSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_override: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<RoleResolutionIssue>,
}

impl ResolvedRoleDefinition {
    pub fn definition_identity(&self) -> Option<(&str, u32)> {
        self.definition
            .as_ref()
            .map(|definition| (definition.id.as_str(), definition.version))
    }
}

#[derive(Debug, Default, Deserialize)]
struct RoleDefinitionOverride {
    id: Option<String>,
    version: Option<u32>,
    domain: Option<String>,
    knowledge_scope: Option<Vec<KnowledgeRef>>,
    lens: Option<RoleLens>,
    authority: Option<AuthorityScope>,
    behavior: Option<CliBehavior>,
    context_boundary: Option<ContextBoundary>,
    signal_class: Option<SignalClass>,
    prompt_template: Option<String>,
    non_goals: Option<Vec<String>>,
}

/// Return the stable template key associated with a declared role. Invalid
/// path-shaped input is made explicit without allowing template traversal.
pub fn role_prompt_template(role_id: &str) -> String {
    normalize_role_id(role_id)
        .map(|id| format!("roles/{id}"))
        .unwrap_or_else(|| "roles/unresolved".to_string())
}

/// Resolve an institutional base plus an optional project override. The
/// project root is an explicit argument by design; resolution never consults
/// the process current directory.
pub fn resolve_role_definition(
    project_path: &Path,
    institutional_wiki_root: Option<&Path>,
    role_id: &str,
) -> ResolvedRoleDefinition {
    let requested_id = role_id.trim().to_ascii_lowercase();
    let Some(normalized_id) = normalize_role_id(role_id) else {
        return ResolvedRoleDefinition {
            requested_id,
            definition: None,
            base_source: None,
            applied_override: None,
            issues: vec![RoleResolutionIssue {
                kind: RoleResolutionIssueKind::DefinitionNotFound,
                source_ref: role_id.to_string(),
                detail: "role id must contain only lowercase letters, digits, and hyphens"
                    .to_string(),
            }],
        };
    };

    let mut issues = Vec::new();
    let (mut definition, base_source) = load_base_definition(
        institutional_wiki_root,
        &normalized_id,
        &mut issues,
    );
    let mut applied_override = None;

    let ai_docs = project_path.join(".ai-docs");
    if !ai_docs.is_dir() {
        issues.push(RoleResolutionIssue {
            kind: RoleResolutionIssueKind::ProjectKnowledgeUnavailable,
            source_ref: ai_docs.display().to_string(),
            detail: "project knowledge directory is unavailable".to_string(),
        });
    } else {
        let override_path = project_path
            .join(PROJECT_ROLE_DIR)
            .join(format!("{normalized_id}.md"));
        if override_path.exists() {
            match fs::read_to_string(&override_path)
                .map_err(|error| error.to_string())
                .and_then(|source| parse_document::<RoleDefinitionOverride>(&source))
                .and_then(|patch| {
                    apply_project_override(definition.clone(), &normalized_id, patch)
                }) {
                Ok(overridden) => {
                    definition = Some(overridden);
                    applied_override = Some(override_path.display().to_string());
                }
                Err(detail) => issues.push(source_unreadable(&override_path, detail)),
            }
        }
    }

    if definition.is_none() {
        issues.push(RoleResolutionIssue {
            kind: RoleResolutionIssueKind::DefinitionNotFound,
            source_ref: normalized_id.clone(),
            detail: "no institutional, embedded, or project role definition resolved".to_string(),
        });
    }

    ResolvedRoleDefinition {
        requested_id: normalized_id,
        definition,
        base_source,
        applied_override,
        issues,
    }
}

fn load_base_definition(
    institutional_wiki_root: Option<&Path>,
    role_id: &str,
    issues: &mut Vec<RoleResolutionIssue>,
) -> (Option<RoleDefinition>, Option<RoleDefinitionSource>) {
    if let Some(root) = institutional_wiki_root {
        let path = root
            .join(INSTITUTIONAL_ROLE_DIR)
            .join(format!("{role_id}.md"));
        if path.exists() {
            match fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|source| parse_role_definition(&source, role_id))
            {
                Ok(definition) => {
                    return (Some(definition), Some(RoleDefinitionSource::Institutional));
                }
                Err(detail) => issues.push(source_unreadable(&path, detail)),
            }
        } else {
            issues.push(RoleResolutionIssue {
                kind: RoleResolutionIssueKind::InstitutionalUnavailable,
                source_ref: path.display().to_string(),
                detail: "institutional role definition is absent".to_string(),
            });
        }
    } else {
        issues.push(RoleResolutionIssue {
            kind: RoleResolutionIssueKind::InstitutionalUnavailable,
            source_ref: INSTITUTIONAL_ROLE_DIR.to_string(),
            detail: "institutional knowledge root is not configured".to_string(),
        });
    }

    let embedded = EMBEDDED_ROLE_DEFINITIONS
        .iter()
        .find(|(id, _)| *id == role_id)
        .map(|(_, source)| *source);
    match embedded {
        Some(source) => match parse_role_definition(source, role_id) {
            Ok(definition) => (
                Some(definition),
                Some(RoleDefinitionSource::EmbeddedDefault),
            ),
            Err(detail) => {
                issues.push(RoleResolutionIssue {
                    kind: RoleResolutionIssueKind::SourceUnreadable,
                    source_ref: format!("embedded:roles/{role_id}.md"),
                    detail,
                });
                (None, None)
            }
        },
        None => (None, None),
    }
}

fn apply_project_override(
    base: Option<RoleDefinition>,
    role_id: &str,
    patch: RoleDefinitionOverride,
) -> Result<RoleDefinition, String> {
    if let Some(id) = patch.id.as_deref() {
        let normalized = normalize_role_id(id)
            .ok_or_else(|| "override contains an invalid role id".to_string())?;
        if normalized != role_id {
            return Err(format!(
                "override declares role {normalized}, expected {role_id}"
            ));
        }
    }
    if base.is_none() && patch.version.is_none() {
        return Err("a project-only role definition must declare a version".to_string());
    }

    let mut definition = base.unwrap_or_else(|| RoleDefinition::empty(role_id));
    definition.id = role_id.to_string();
    if let Some(value) = patch.version {
        definition.version = value;
    }
    if let Some(value) = patch.domain {
        definition.domain = Some(value);
    }
    if let Some(value) = patch.knowledge_scope {
        definition.knowledge_scope = value;
    }
    if let Some(value) = patch.lens {
        definition.lens = Some(value);
    }
    if let Some(value) = patch.authority {
        definition.authority = value;
    }
    if let Some(value) = patch.behavior {
        definition.behavior = Some(value);
    }
    if let Some(value) = patch.context_boundary {
        definition.context_boundary = value;
    }
    if let Some(value) = patch.signal_class {
        definition.signal_class = Some(value);
    }
    if let Some(value) = patch.prompt_template {
        definition.prompt_template = Some(value);
    }
    if let Some(value) = patch.non_goals {
        definition.non_goals = value;
    }
    if definition.prompt_template.is_none() {
        definition.prompt_template = Some(role_prompt_template(role_id));
    }
    validate_role_definition(&definition, role_id)?;
    Ok(definition)
}

fn parse_role_definition(source: &str, role_id: &str) -> Result<RoleDefinition, String> {
    let mut definition: RoleDefinition = parse_document(source)?;
    let normalized = normalize_role_id(&definition.id)
        .ok_or_else(|| "definition contains an invalid role id".to_string())?;
    if normalized != role_id {
        return Err(format!(
            "definition declares role {normalized}, expected {role_id}"
        ));
    }
    definition.id = normalized;
    validate_role_definition(&definition, role_id)?;
    Ok(definition)
}

fn parse_document<T: for<'de> Deserialize<'de>>(source: &str) -> Result<T, String> {
    let trimmed = source.trim();
    let json = if let Some(after_open) = trimmed.strip_prefix("---") {
        let (front_matter, _) = after_open
            .split_once("---")
            .ok_or_else(|| "role definition front matter is missing its closing ---".to_string())?;
        front_matter.trim()
    } else {
        trimmed
    };
    serde_json::from_str(json).map_err(|error| error.to_string())
}

fn validate_role_definition(definition: &RoleDefinition, expected_id: &str) -> Result<(), String> {
    if definition.id != expected_id {
        return Err(format!(
            "definition id {} does not match {expected_id}",
            definition.id
        ));
    }
    if definition.version == 0 {
        return Err("resolved definitions must have a non-zero version".to_string());
    }
    for reference in &definition.knowledge_scope {
        let pointer_len = reference.pointer.chars().count();
        if reference.pointer.trim().is_empty() || pointer_len > MAX_KNOWLEDGE_POINTER_CHARS {
            return Err(format!(
                "knowledge pointer must contain 1..={MAX_KNOWLEDGE_POINTER_CHARS} characters"
            ));
        }
        if reference
            .summary
            .as_deref()
            .is_some_and(|summary| summary.chars().count() > MAX_KNOWLEDGE_SUMMARY_CHARS)
        {
            return Err(format!(
                "knowledge summary exceeds {MAX_KNOWLEDGE_SUMMARY_CHARS} characters"
            ));
        }
    }
    Ok(())
}

fn normalize_role_id(role_id: &str) -> Option<String> {
    let normalized = role_id.trim().to_ascii_lowercase();
    (!normalized.is_empty()
        && normalized.len() <= 64
        && normalized
            .chars()
            .all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '-'
            }))
    .then_some(normalized)
}

fn source_unreadable(path: &Path, detail: String) -> RoleResolutionIssue {
    RoleResolutionIssue {
        kind: RoleResolutionIssueKind::SourceUnreadable,
        source_ref: path.display().to_string(),
        detail,
    }
}
