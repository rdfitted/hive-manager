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
    let (model, flags, cost_rank): (&str, &[&str], u8) = match (provider, preset_id) {
        ("claude", "claude-haiku-4-5") => ("claude-haiku-4-5", &[], 0),
        ("claude", "claude-sonnet-4-6") => ("claude-sonnet-4-6", &[], 1),
        ("claude", "opus-high") => ("opus", &["--settings", "{\"effortLevel\":\"high\"}"], 2),
        ("claude", "fable-high") => ("fable", &["--settings", "{\"effortLevel\":\"high\"}"], 3),
        ("codex", "codex-gpt-5-6-terra-medium") => (
            "gpt-5.6-terra",
            &["-c", "model_reasoning_effort=\"medium\""],
            0,
        ),
        ("codex", "codex-gpt-5-6-sol-medium") => (
            "gpt-5.6-sol",
            &["-c", "model_reasoning_effort=\"medium\""],
            1,
        ),
        ("codex", "codex-gpt-5-6-sol-xhigh") => (
            "gpt-5.6-sol",
            &["-c", "model_reasoning_effort=\"xhigh\""],
            2,
        ),
        ("codex", "codex-gpt-5-6-sol-max") => {
            ("gpt-5.6-sol", &["-c", "model_reasoning_effort=\"max\""], 3)
        }
        _ => return None,
    };

    Some(PresetExpansion {
        resolved: ResolvedTier {
            model: model.to_string(),
            flags: flags.iter().map(|flag| (*flag).to_string()).collect(),
        },
        cost_rank,
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
    fn rust_expansions_match_the_eight_frontend_apply_preset_cases() {
        let source = include_str!("../../../src/lib/components/AgentConfigEditor.svelte")
            .replace("\r\n", "\n");
        let parity_cases = [
            (
                "claude",
                "claude-haiku-4-5",
                "claude-haiku-4-5",
                Vec::<&str>::new(),
                "      case 'claude-haiku-4-5':\n        model = 'claude-haiku-4-5';\n        break;",
            ),
            (
                "claude",
                "claude-sonnet-4-6",
                "claude-sonnet-4-6",
                vec![],
                "      case 'claude-sonnet-4-6':\n        model = 'claude-sonnet-4-6';\n        break;",
            ),
            (
                "claude",
                "opus-high",
                "opus",
                vec!["--settings", "{\"effortLevel\":\"high\"}"],
                "      case 'opus-high':\n        model = 'opus';\n        flags.push('--settings', JSON.stringify({ effortLevel: 'high' }));\n        break;",
            ),
            (
                "claude",
                "fable-high",
                "fable",
                vec!["--settings", "{\"effortLevel\":\"high\"}"],
                "      case 'fable-high':\n        model = 'fable';\n        flags.push('--settings', JSON.stringify({ effortLevel: 'high' }));\n        break;",
            ),
            (
                "codex",
                "codex-gpt-5-6-terra-medium",
                "gpt-5.6-terra",
                vec!["-c", "model_reasoning_effort=\"medium\""],
                "      case 'codex-gpt-5-6-terra-medium':\n        model = 'gpt-5.6-terra';\n        flags.push('-c', 'model_reasoning_effort=\"medium\"');\n        break;",
            ),
            (
                "codex",
                "codex-gpt-5-6-sol-medium",
                "gpt-5.6-sol",
                vec!["-c", "model_reasoning_effort=\"medium\""],
                "      case 'codex-gpt-5-6-sol-medium':\n        model = 'gpt-5.6-sol';\n        flags.push('-c', 'model_reasoning_effort=\"medium\"');\n        break;",
            ),
            (
                "codex",
                "codex-gpt-5-6-sol-xhigh",
                "gpt-5.6-sol",
                vec!["-c", "model_reasoning_effort=\"xhigh\""],
                "      case 'codex-gpt-5-6-sol-xhigh':\n        model = 'gpt-5.6-sol';\n        flags.push('-c', 'model_reasoning_effort=\"xhigh\"');\n        break;",
            ),
            (
                "codex",
                "codex-gpt-5-6-sol-max",
                "gpt-5.6-sol",
                vec!["-c", "model_reasoning_effort=\"max\""],
                "      case 'codex-gpt-5-6-sol-max':\n        model = 'gpt-5.6-sol';\n        flags.push('-c', 'model_reasoning_effort=\"max\"');\n        break;",
            ),
        ];

        for (provider, preset_id, model, flags, frontend_case) in parity_cases {
            let rust = expand_preset(provider, preset_id).unwrap().resolved;
            assert_eq!(rust.model, model, "model drift for {preset_id}");
            assert_eq!(rust.flags, flags, "flag drift for {preset_id}");
            assert!(
                source.contains(frontend_case),
                "AgentConfigEditor applyPreset drifted for {preset_id}"
            );
        }
    }
}
