//! Maintained per-provider task-tier resolution.
//!
//! The institutional wiki is the preferred base. The checked-in document is
//! embedded as an installation-safe fallback, and a project may patch
//! individual cells under `.ai-docs` without replacing the whole ladder.

use std::fs;
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::orchestrator::work_graph::schema::TaskTier;

pub const INSTITUTIONAL_TIER_LADDER_PATH: &str = "tiers/ladder.md";
pub const PROJECT_TIER_LADDER_PATH: &str = ".ai-docs/tiers/ladder.md";

const EMBEDDED_TIER_LADDER: &str = include_str!("../../../tiers/ladder.md");

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ProviderTierLadder {
    pub low: String,
    pub medium: String,
    pub high: String,
    pub critical: String,
}

impl ProviderTierLadder {
    fn preset_id(&self, tier: TaskTier) -> &str {
        match tier {
            TaskTier::Low => &self.low,
            TaskTier::Medium => &self.medium,
            TaskTier::High => &self.high,
            TaskTier::Critical => &self.critical,
        }
    }

    fn cells(&self) -> [(&'static str, &str); 4] {
        [
            ("low", &self.low),
            ("medium", &self.medium),
            ("high", &self.high),
            ("critical", &self.critical),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct TierLadder {
    pub claude: ProviderTierLadder,
    pub codex: ProviderTierLadder,
}

impl TierLadder {
    /// Return the maintained preset id for a supported provider and task tier.
    pub fn preset_id(&self, provider: &str, tier: TaskTier) -> Option<&str> {
        match provider {
            "claude" => Some(self.claude.preset_id(tier)),
            "codex" => Some(self.codex.preset_id(tier)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedTier {
    pub model: String,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TierLadderSource {
    Institutional,
    EmbeddedDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TierLadderResolutionIssueKind {
    InstitutionalUnavailable,
    ProjectKnowledgeUnavailable,
    SourceUnreadable,
    UnknownPreset,
    NonMonotone,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct TierLadderResolutionIssue {
    pub kind: TierLadderResolutionIssueKind,
    pub source_ref: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedTierLadder {
    pub ladder: Option<TierLadder>,
    pub base_source: Option<TierLadderSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_override: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<TierLadderResolutionIssue>,
}

impl ResolvedTierLadder {
    /// Expand one cell from the fully layered ladder into provider-native launch
    /// arguments. Unsupported providers and an unavailable base return `None`.
    pub fn resolve_tier(&self, provider: &str, tier: TaskTier) -> Option<ResolvedTier> {
        let preset_id = self.ladder.as_ref()?.preset_id(provider, tier)?;
        expand_preset(provider, preset_id).map(|preset| preset.resolved)
    }
}

#[derive(Debug, Deserialize)]
struct TierLadderDocument {
    tier_ladder: TierLadder,
}

#[derive(Debug, Default, Deserialize)]
struct TierLadderDocumentPatch {
    tier_ladder: TierLadderPatch,
}

#[derive(Debug, Default, Deserialize)]
struct TierLadderPatch {
    claude: Option<ProviderTierLadderPatch>,
    codex: Option<ProviderTierLadderPatch>,
}

#[derive(Debug, Default, Deserialize)]
struct ProviderTierLadderPatch {
    low: Option<String>,
    medium: Option<String>,
    high: Option<String>,
    critical: Option<String>,
}

#[derive(Debug)]
pub(crate) struct PresetExpansion {
    pub(crate) resolved: ResolvedTier,
    pub(crate) cost_rank: u8,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub(crate) struct PresetDefinition {
    pub(crate) provider: &'static str,
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) model: &'static str,
    pub(crate) flags: &'static [&'static str],
    #[serde(skip)]
    cost_rank: u8,
}

macro_rules! preset {
    ($provider:literal, $id:literal, $label:literal, $model:literal, $cost_rank:literal $(, $flag:literal)*) => {
        PresetDefinition {
            provider: $provider,
            id: $id,
            label: $label,
            model: $model,
            flags: &[$($flag),*],
            cost_rank: $cost_rank,
        }
    };
}

// This is the single operator-visible preset catalogue. `cost_rank` remains
// server-only metadata for tier-ladder monotonicity checks; it is deliberately
// explicit rather than inferred from the UI display order.
const PRESET_CATALOGUE: &[PresetDefinition] = &[
    preset!(
        "claude",
        "fable-high",
        "Fable 5 (High effort)",
        "fable",
        3,
        "--settings",
        "{\"effortLevel\":\"high\"}"
    ),
    preset!(
        "claude",
        "fable-max",
        "Fable 5 (Max effort)",
        "fable",
        3,
        "--settings",
        "{\"effortLevel\":\"max\"}"
    ),
    preset!("claude", "fable", "Fable 5", "fable", 3),
    preset!(
        "claude",
        "opus-high",
        "Opus (High effort)",
        "opus",
        2,
        "--settings",
        "{\"effortLevel\":\"high\"}"
    ),
    preset!(
        "claude",
        "opus-low",
        "Opus (Low effort)",
        "opus",
        2,
        "--settings",
        "{\"effortLevel\":\"low\"}"
    ),
    preset!("claude", "opus", "Opus", "opus", 2),
    preset!(
        "claude",
        "claude-opus-4-6-high",
        "Opus 4.6 (High effort)",
        "claude-opus-4-6",
        2,
        "--settings",
        "{\"effortLevel\":\"high\"}"
    ),
    preset!(
        "claude",
        "claude-opus-4-6-low",
        "Opus 4.6 (Low effort)",
        "claude-opus-4-6",
        2,
        "--settings",
        "{\"effortLevel\":\"low\"}"
    ),
    preset!(
        "claude",
        "claude-opus-4-5",
        "Opus 4.5",
        "claude-opus-4-5",
        2
    ),
    preset!(
        "claude",
        "claude-sonnet-4-6",
        "Sonnet 4.6",
        "claude-sonnet-4-6",
        1
    ),
    preset!(
        "claude",
        "claude-sonnet-4-5",
        "Sonnet 4.5",
        "claude-sonnet-4-5-20250929",
        1
    ),
    preset!(
        "claude",
        "claude-haiku-4-5",
        "Haiku 4.5",
        "claude-haiku-4-5",
        0
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-sol",
        "GPT-5.6 Sol",
        "gpt-5.6-sol",
        1
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-sol-low",
        "GPT-5.6 Sol (Low effort)",
        "gpt-5.6-sol",
        1,
        "-c",
        "model_reasoning_effort=\"low\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-sol-medium",
        "GPT-5.6 Sol (Medium effort)",
        "gpt-5.6-sol",
        1,
        "-c",
        "model_reasoning_effort=\"medium\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-sol-high",
        "GPT-5.6 Sol (High effort)",
        "gpt-5.6-sol",
        2,
        "-c",
        "model_reasoning_effort=\"high\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-sol-xhigh",
        "GPT-5.6 Sol (Extra high effort)",
        "gpt-5.6-sol",
        2,
        "-c",
        "model_reasoning_effort=\"xhigh\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-sol-max",
        "GPT-5.6 Sol (Max effort)",
        "gpt-5.6-sol",
        3,
        "-c",
        "model_reasoning_effort=\"max\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-sol-ultra",
        "GPT-5.6 Sol (Ultra effort)",
        "gpt-5.6-sol",
        3,
        "-c",
        "model_reasoning_effort=\"ultra\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-terra",
        "GPT-5.6 Terra",
        "gpt-5.6-terra",
        0
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-terra-low",
        "GPT-5.6 Terra (Low effort)",
        "gpt-5.6-terra",
        0,
        "-c",
        "model_reasoning_effort=\"low\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-terra-medium",
        "GPT-5.6 Terra (Medium effort)",
        "gpt-5.6-terra",
        0,
        "-c",
        "model_reasoning_effort=\"medium\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-terra-high",
        "GPT-5.6 Terra (High effort)",
        "gpt-5.6-terra",
        0,
        "-c",
        "model_reasoning_effort=\"high\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-luna",
        "GPT-5.6 Luna",
        "gpt-5.6-luna",
        0
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-luna-low",
        "GPT-5.6 Luna (Low effort)",
        "gpt-5.6-luna",
        0,
        "-c",
        "model_reasoning_effort=\"low\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-luna-medium",
        "GPT-5.6 Luna (Medium effort)",
        "gpt-5.6-luna",
        0,
        "-c",
        "model_reasoning_effort=\"medium\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-6-luna-high",
        "GPT-5.6 Luna (High effort)",
        "gpt-5.6-luna",
        0,
        "-c",
        "model_reasoning_effort=\"high\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-5-low",
        "GPT-5.5 (Low effort)",
        "gpt-5.5",
        0,
        "-c",
        "model_reasoning_effort=\"low\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-5-medium",
        "GPT-5.5 (Medium effort)",
        "gpt-5.5",
        1,
        "-c",
        "model_reasoning_effort=\"medium\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-5-high",
        "GPT-5.5 (High effort)",
        "gpt-5.5",
        2,
        "-c",
        "model_reasoning_effort=\"high\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-5-xhigh",
        "GPT-5.5 (Extra high effort)",
        "gpt-5.5",
        2,
        "-c",
        "model_reasoning_effort=\"xhigh\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-4-low",
        "GPT-5.4 (Low effort)",
        "gpt-5.4",
        0,
        "-c",
        "model_reasoning_effort=\"low\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-4-medium",
        "GPT-5.4 (Medium effort)",
        "gpt-5.4",
        1,
        "-c",
        "model_reasoning_effort=\"medium\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-4-high",
        "GPT-5.4 (High effort)",
        "gpt-5.4",
        2,
        "-c",
        "model_reasoning_effort=\"high\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-4-xhigh",
        "GPT-5.4 (Extra high effort)",
        "gpt-5.4",
        2,
        "-c",
        "model_reasoning_effort=\"xhigh\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-3-low",
        "GPT-5.3 Codex (Low effort)",
        "gpt-5.3-codex",
        0,
        "-c",
        "model_reasoning_effort=\"low\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-3-medium",
        "GPT-5.3 Codex (Medium effort)",
        "gpt-5.3-codex",
        1,
        "-c",
        "model_reasoning_effort=\"medium\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-3-high",
        "GPT-5.3 Codex (High effort)",
        "gpt-5.3-codex",
        2,
        "-c",
        "model_reasoning_effort=\"high\""
    ),
    preset!(
        "codex",
        "codex-gpt-5-3-xhigh",
        "GPT-5.3 Codex (Extra high effort)",
        "gpt-5.3-codex",
        2,
        "-c",
        "model_reasoning_effort=\"xhigh\""
    ),
    preset!(
        "cursor",
        "composer-2.5",
        "Composer 2.5 (latest)",
        "composer-2.5",
        0
    ),
    preset!("cursor", "composer-2", "Composer 2.0", "composer-2", 0),
    preset!(
        "cursor",
        "composer-2-fast",
        "Composer 2.0 Fast",
        "composer-2-fast",
        0
    ),
    preset!("cursor", "composer-1", "Composer 1", "composer-1", 0),
    preset!("droid", "glm-5.1", "GLM 5.1", "glm-5.1", 0),
    preset!("droid", "glm-4.7", "GLM 4.7", "glm-4.7", 0),
    preset!(
        "opencode",
        "opencode/big-pickle",
        "BigPickle",
        "opencode/big-pickle",
        0
    ),
    preset!("opencode", "opencode/grok", "Grok", "opencode/grok", 0),
    preset!("qwen", "qwen3-coder", "Qwen3 Coder", "qwen3-coder", 0),
    preset!("qwen", "qwen2.5-coder", "Qwen2.5 Coder", "qwen2.5-coder", 0),
];

pub(crate) fn preset_catalogue() -> &'static [PresetDefinition] {
    PRESET_CATALOGUE
}

#[derive(Debug)]
struct LadderLoadError {
    kind: TierLadderResolutionIssueKind,
    detail: String,
}

/// Resolve the institutional base (or embedded fallback) and then apply a
/// project-local, per-cell patch. The project root is always explicit; this
/// function never consults the process current directory.
pub fn resolve_tier_ladder(
    project_path: &Path,
    institutional_wiki_root: Option<&Path>,
) -> ResolvedTierLadder {
    let mut issues = Vec::new();
    let (mut ladder, base_source) = load_base_ladder(institutional_wiki_root, &mut issues);
    let mut applied_override = None;

    let ai_docs = project_path.join(".ai-docs");
    if !ai_docs.is_dir() {
        issues.push(TierLadderResolutionIssue {
            kind: TierLadderResolutionIssueKind::ProjectKnowledgeUnavailable,
            source_ref: ai_docs.display().to_string(),
            detail: "project knowledge directory is unavailable".to_string(),
        });
    } else {
        let override_path = project_path.join(PROJECT_TIER_LADDER_PATH);
        if override_path.exists() {
            match fs::read_to_string(&override_path)
                .map_err(|error| error.to_string())
                .and_then(|source| parse_document::<TierLadderDocumentPatch>(&source))
            {
                Ok(patch) => {
                    if let Some(ref mut resolved) = ladder {
                        apply_project_override(
                            resolved,
                            patch.tier_ladder,
                            &override_path,
                            &mut issues,
                        );
                        applied_override = Some(override_path.display().to_string());
                    }
                }
                Err(detail) => issues.push(source_unreadable(&override_path, detail)),
            }
        }
    }

    if let Some(ref resolved) = ladder {
        let source_ref = applied_override
            .clone()
            .unwrap_or_else(|| match base_source {
                Some(TierLadderSource::Institutional) => institutional_wiki_root
                    .map(|root| root.join(INSTITUTIONAL_TIER_LADDER_PATH))
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| INSTITUTIONAL_TIER_LADDER_PATH.to_string()),
                _ => format!("embedded:{INSTITUTIONAL_TIER_LADDER_PATH}"),
            });
        warn_if_non_monotone(resolved, &source_ref, &mut issues);
    }

    ResolvedTierLadder {
        ladder,
        base_source,
        applied_override,
        issues,
    }
}

/// Parse the checked-in default. Kept fallible so malformed maintained config
/// can never turn tier resolution into a process panic.
pub fn embedded_tier_ladder() -> Result<TierLadder, String> {
    parse_ladder(EMBEDDED_TIER_LADDER).map_err(|error| error.detail)
}

pub(crate) fn embedded_resolved_tier_ladder() -> ResolvedTierLadder {
    match parse_ladder(EMBEDDED_TIER_LADDER) {
        Ok(ladder) => ResolvedTierLadder {
            ladder: Some(ladder),
            base_source: Some(TierLadderSource::EmbeddedDefault),
            applied_override: None,
            issues: Vec::new(),
        },
        Err(error) => ResolvedTierLadder {
            ladder: None,
            base_source: None,
            applied_override: None,
            issues: vec![TierLadderResolutionIssue {
                kind: error.kind,
                source_ref: format!("embedded:{INSTITUTIONAL_TIER_LADDER_PATH}"),
                detail: error.detail,
            }],
        },
    }
}

fn load_base_ladder(
    institutional_wiki_root: Option<&Path>,
    issues: &mut Vec<TierLadderResolutionIssue>,
) -> (Option<TierLadder>, Option<TierLadderSource>) {
    if let Some(root) = institutional_wiki_root {
        let path = root.join(INSTITUTIONAL_TIER_LADDER_PATH);
        if path.exists() {
            match fs::read_to_string(&path)
                .map_err(|error| LadderLoadError {
                    kind: TierLadderResolutionIssueKind::SourceUnreadable,
                    detail: error.to_string(),
                })
                .and_then(|source| parse_ladder(&source))
            {
                Ok(ladder) => {
                    return (Some(ladder), Some(TierLadderSource::Institutional));
                }
                Err(error) => issues.push(TierLadderResolutionIssue {
                    kind: error.kind,
                    source_ref: path.display().to_string(),
                    detail: error.detail,
                }),
            }
        } else {
            issues.push(TierLadderResolutionIssue {
                kind: TierLadderResolutionIssueKind::InstitutionalUnavailable,
                source_ref: path.display().to_string(),
                detail: "institutional tier ladder is absent".to_string(),
            });
        }
    } else {
        issues.push(TierLadderResolutionIssue {
            kind: TierLadderResolutionIssueKind::InstitutionalUnavailable,
            source_ref: INSTITUTIONAL_TIER_LADDER_PATH.to_string(),
            detail: "institutional knowledge root is not configured".to_string(),
        });
    }

    match parse_ladder(EMBEDDED_TIER_LADDER) {
        Ok(ladder) => (Some(ladder), Some(TierLadderSource::EmbeddedDefault)),
        Err(error) => {
            issues.push(TierLadderResolutionIssue {
                kind: error.kind,
                source_ref: format!("embedded:{INSTITUTIONAL_TIER_LADDER_PATH}"),
                detail: error.detail,
            });
            (None, None)
        }
    }
}

fn parse_ladder(source: &str) -> Result<TierLadder, LadderLoadError> {
    let document =
        parse_document::<TierLadderDocument>(source).map_err(|detail| LadderLoadError {
            kind: TierLadderResolutionIssueKind::SourceUnreadable,
            detail,
        })?;
    validate_provider("claude", &document.tier_ladder.claude)?;
    validate_provider("codex", &document.tier_ladder.codex)?;
    Ok(document.tier_ladder)
}

fn validate_provider(provider: &str, ladder: &ProviderTierLadder) -> Result<(), LadderLoadError> {
    for (tier, preset_id) in ladder.cells() {
        if expand_preset(provider, preset_id).is_none() {
            return Err(LadderLoadError {
                kind: TierLadderResolutionIssueKind::UnknownPreset,
                detail: format!(
                    "tier_ladder.{provider}.{tier} names unknown preset id {preset_id:?}"
                ),
            });
        }
    }
    Ok(())
}

fn apply_project_override(
    ladder: &mut TierLadder,
    patch: TierLadderPatch,
    source_path: &Path,
    issues: &mut Vec<TierLadderResolutionIssue>,
) {
    if let Some(provider_patch) = patch.claude {
        apply_provider_patch(
            "claude",
            &mut ladder.claude,
            provider_patch,
            source_path,
            issues,
        );
    }
    if let Some(provider_patch) = patch.codex {
        apply_provider_patch(
            "codex",
            &mut ladder.codex,
            provider_patch,
            source_path,
            issues,
        );
    }
}

fn apply_provider_patch(
    provider: &str,
    ladder: &mut ProviderTierLadder,
    patch: ProviderTierLadderPatch,
    source_path: &Path,
    issues: &mut Vec<TierLadderResolutionIssue>,
) {
    apply_cell(
        provider,
        "low",
        &mut ladder.low,
        patch.low,
        source_path,
        issues,
    );
    apply_cell(
        provider,
        "medium",
        &mut ladder.medium,
        patch.medium,
        source_path,
        issues,
    );
    apply_cell(
        provider,
        "high",
        &mut ladder.high,
        patch.high,
        source_path,
        issues,
    );
    apply_cell(
        provider,
        "critical",
        &mut ladder.critical,
        patch.critical,
        source_path,
        issues,
    );
}

fn apply_cell(
    provider: &str,
    tier: &str,
    current: &mut String,
    replacement: Option<String>,
    source_path: &Path,
    issues: &mut Vec<TierLadderResolutionIssue>,
) {
    let Some(replacement) = replacement else {
        return;
    };
    if expand_preset(provider, &replacement).is_some() {
        *current = replacement;
        return;
    }

    let issue = TierLadderResolutionIssue {
        kind: TierLadderResolutionIssueKind::UnknownPreset,
        source_ref: format!("{}#tier_ladder.{provider}.{tier}", source_path.display()),
        detail: format!(
            "unknown preset id {replacement:?}; retaining institutional value {current:?}"
        ),
    };
    tracing::warn!(
        source_ref = %issue.source_ref,
        detail = %issue.detail,
        "tier ladder override contains an unknown preset"
    );
    issues.push(issue);
}

fn warn_if_non_monotone(
    ladder: &TierLadder,
    source_ref: &str,
    issues: &mut Vec<TierLadderResolutionIssue>,
) {
    for (provider, provider_ladder) in [("claude", &ladder.claude), ("codex", &ladder.codex)] {
        let cells = provider_ladder.cells();
        let ranks = cells.map(|(_, preset_id)| {
            expand_preset(provider, preset_id)
                .map(|preset| preset.cost_rank)
                .unwrap_or_default()
        });
        if ranks.windows(2).any(|pair| pair[0] > pair[1]) {
            let issue = TierLadderResolutionIssue {
                kind: TierLadderResolutionIssueKind::NonMonotone,
                source_ref: source_ref.to_string(),
                detail: format!(
                    "tier_ladder.{provider} decreases in maintained preset order; the ladder is allowed but may route a higher tier to a lower-cost preset"
                ),
            };
            tracing::warn!(
                source_ref = %issue.source_ref,
                detail = %issue.detail,
                "tier ladder is not monotone"
            );
            issues.push(issue);
        }
    }
}

fn parse_document<T: for<'de> Deserialize<'de>>(source: &str) -> Result<T, String> {
    let trimmed = source.trim();
    let json = if let Some(after_open) = trimmed.strip_prefix("---") {
        let (front_matter, _) = after_open
            .split_once("---")
            .ok_or_else(|| "tier ladder front matter is missing its closing ---".to_string())?;
        front_matter.trim()
    } else {
        trimmed
    };
    serde_json::from_str(json).map_err(|error| error.to_string())
}

fn source_unreadable(path: &Path, detail: String) -> TierLadderResolutionIssue {
    TierLadderResolutionIssue {
        kind: TierLadderResolutionIssueKind::SourceUnreadable,
        source_ref: path.display().to_string(),
        detail,
    }
}

pub(crate) fn expand_preset(provider: &str, preset_id: &str) -> Option<PresetExpansion> {
    let preset = PRESET_CATALOGUE
        .iter()
        .find(|preset| preset.provider == provider && preset.id == preset_id)?;

    Some(PresetExpansion {
        resolved: ResolvedTier {
            model: preset.model.to_string(),
            flags: preset
                .flags
                .iter()
                .map(|flag| (*flag).to_string())
                .collect(),
        },
        cost_rank: preset.cost_rank,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const TIERS: [TaskTier; 4] = [
        TaskTier::Low,
        TaskTier::Medium,
        TaskTier::High,
        TaskTier::Critical,
    ];

    fn write_override(project: &Path, front_matter: &str) {
        let directory = project.join(".ai-docs/tiers");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("ladder.md"),
            format!("---\n{front_matter}\n---\n"),
        )
        .unwrap();
    }

    #[test]
    fn embedded_ladder_resolves_all_eight_provider_native_cells() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join(".ai-docs")).unwrap();
        let resolved = resolve_tier_ladder(project.path(), None);

        let expected = [
            (
                "claude",
                TaskTier::Low,
                "claude-haiku-4-5",
                Vec::<&str>::new(),
            ),
            ("claude", TaskTier::Medium, "claude-sonnet-4-6", vec![]),
            (
                "claude",
                TaskTier::High,
                "opus",
                vec!["--settings", "{\"effortLevel\":\"high\"}"],
            ),
            (
                "claude",
                TaskTier::Critical,
                "fable",
                vec!["--settings", "{\"effortLevel\":\"high\"}"],
            ),
            (
                "codex",
                TaskTier::Low,
                "gpt-5.6-terra",
                vec!["-c", "model_reasoning_effort=\"medium\""],
            ),
            (
                "codex",
                TaskTier::Medium,
                "gpt-5.6-sol",
                vec!["-c", "model_reasoning_effort=\"medium\""],
            ),
            (
                "codex",
                TaskTier::High,
                "gpt-5.6-sol",
                vec!["-c", "model_reasoning_effort=\"xhigh\""],
            ),
            (
                "codex",
                TaskTier::Critical,
                "gpt-5.6-sol",
                vec!["-c", "model_reasoning_effort=\"max\""],
            ),
        ];

        for (provider, tier, model, flags) in expected {
            let cell = resolved.resolve_tier(provider, tier).unwrap();
            assert_eq!(cell.model, model);
            assert_eq!(cell.flags, flags);
        }
        assert!(resolved.resolve_tier("droid", TaskTier::Medium).is_none());
    }

    #[test]
    fn project_override_replaces_only_one_cell() {
        let project = tempfile::tempdir().unwrap();
        write_override(
            project.path(),
            r#"{
              "tier_ladder": {
                "codex": { "low": "codex-gpt-5-6-sol-medium" }
              }
            }"#,
        );

        let baseline = embedded_tier_ladder().unwrap();
        let resolved = resolve_tier_ladder(project.path(), None);
        let ladder = resolved.ladder.as_ref().unwrap();

        assert_eq!(ladder.codex.low, "codex-gpt-5-6-sol-medium");
        for provider in ["claude", "codex"] {
            for tier in TIERS {
                if provider == "codex" && tier == TaskTier::Low {
                    continue;
                }
                assert_eq!(
                    ladder.preset_id(provider, tier),
                    baseline.preset_id(provider, tier),
                    "unexpected change to {provider}.{tier:?}"
                );
            }
        }
    }

    #[test]
    fn unknown_project_preset_warns_and_falls_back_without_panicking() {
        let project = tempfile::tempdir().unwrap();
        write_override(
            project.path(),
            r#"{
              "tier_ladder": {
                "codex": { "low": "codex-invented-ultra" }
              }
            }"#,
        );

        let resolved = resolve_tier_ladder(project.path(), None);
        let low = resolved.resolve_tier("codex", TaskTier::Low).unwrap();

        assert_eq!(low.model, "gpt-5.6-terra");
        assert_eq!(low.flags, vec!["-c", "model_reasoning_effort=\"medium\""]);
        assert!(resolved.issues.iter().any(|issue| {
            issue.kind == TierLadderResolutionIssueKind::UnknownPreset
                && issue.source_ref.ends_with("#tier_ladder.codex.low")
                && issue.detail.contains("retaining institutional value")
        }));
    }

    #[test]
    fn institutional_ladder_precedes_embedded_default() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join(".ai-docs")).unwrap();
        let institutional = tempfile::tempdir().unwrap();
        let tiers = institutional.path().join("tiers");
        fs::create_dir(&tiers).unwrap();
        let source = EMBEDDED_TIER_LADDER.replace(
            "\"low\": \"codex-gpt-5-6-terra-medium\"",
            "\"low\": \"codex-gpt-5-6-sol-medium\"",
        );
        fs::write(tiers.join("ladder.md"), source).unwrap();

        let resolved = resolve_tier_ladder(project.path(), Some(institutional.path()));

        assert_eq!(resolved.base_source, Some(TierLadderSource::Institutional));
        assert_eq!(
            resolved.ladder.as_ref().unwrap().codex.low,
            "codex-gpt-5-6-sol-medium"
        );
    }

    #[test]
    fn catalogue_covers_all_49_unique_presets_and_every_entry_expands() {
        let mut ids = std::collections::HashSet::new();
        let mut provider_counts = std::collections::BTreeMap::new();

        for preset in preset_catalogue() {
            assert!(
                ids.insert(preset.id),
                "duplicate preset id in catalogue: {}",
                preset.id
            );
            *provider_counts.entry(preset.provider).or_insert(0usize) += 1;

            let expansion = expand_preset(preset.provider, preset.id)
                .unwrap_or_else(|| panic!("catalogue entry did not expand: {}", preset.id));
            assert_eq!(expansion.resolved.model, preset.model);
            assert_eq!(expansion.resolved.flags, preset.flags.to_vec());
        }

        assert_eq!(preset_catalogue().len(), 49);
        assert_eq!(
            provider_counts,
            std::collections::BTreeMap::from([
                ("claude", 12),
                ("codex", 27),
                ("cursor", 4),
                ("droid", 2),
                ("opencode", 2),
                ("qwen", 2),
            ])
        );
        assert!(ids.contains("codex-gpt-5-5-xhigh"));
        assert!(ids.contains("codex-gpt-5-4-xhigh"));
        assert!(ids.contains("codex-gpt-5-3-xhigh"));
    }

    #[test]
    fn original_eight_expansions_remain_byte_identical() {
        let expected = [
            ("claude", "claude-haiku-4-5", "claude-haiku-4-5", vec![], 0),
            (
                "claude",
                "claude-sonnet-4-6",
                "claude-sonnet-4-6",
                vec![],
                1,
            ),
            (
                "claude",
                "opus-high",
                "opus",
                vec!["--settings", "{\"effortLevel\":\"high\"}"],
                2,
            ),
            (
                "claude",
                "fable-high",
                "fable",
                vec!["--settings", "{\"effortLevel\":\"high\"}"],
                3,
            ),
            (
                "codex",
                "codex-gpt-5-6-terra-medium",
                "gpt-5.6-terra",
                vec!["-c", "model_reasoning_effort=\"medium\""],
                0,
            ),
            (
                "codex",
                "codex-gpt-5-6-sol-medium",
                "gpt-5.6-sol",
                vec!["-c", "model_reasoning_effort=\"medium\""],
                1,
            ),
            (
                "codex",
                "codex-gpt-5-6-sol-xhigh",
                "gpt-5.6-sol",
                vec!["-c", "model_reasoning_effort=\"xhigh\""],
                2,
            ),
            (
                "codex",
                "codex-gpt-5-6-sol-max",
                "gpt-5.6-sol",
                vec!["-c", "model_reasoning_effort=\"max\""],
                3,
            ),
        ];

        for (provider, id, model, flags, cost_rank) in expected {
            let expansion = expand_preset(provider, id).unwrap();
            assert_eq!(expansion.resolved.model, model, "model drift for {id}");
            assert_eq!(expansion.resolved.flags, flags, "flag drift for {id}");
            assert_eq!(expansion.cost_rank, cost_rank, "rank drift for {id}");
        }
    }
}
