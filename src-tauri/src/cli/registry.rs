use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::domain::{CapabilityCard, CapabilitySupport, DelegationPolicy, NativeDelegationMode};
use crate::orchestrator::org_graph::RoleDefinition;
use crate::orchestrator::work_graph::schema::TaskTier;
use crate::pty::AgentConfig;
use crate::storage::{AppConfig, CliConfig};

use super::tier_ladder::{
    embedded_resolved_tier_ladder, expand_preset, resolve_tier_ladder, ResolvedTier,
    ResolvedTierLadder, TierLadder,
};

/// CLI behavioral profiles for characterizing how different CLI tools behave
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliBehavior {
    /// Highly proactive - will "help" by taking action. Needs strong constraints.
    ActionProne,
    /// Follows instructions literally. Respects role boundaries naturally.
    InstructionFollowing,
    /// Needs explicit bash loops to enforce waiting.
    ExplicitPolling,
    /// Interactive TUI mode - different prompt injection.
    Interactive,
}

/// CLI Registry for building commands from agent configurations
pub struct CliRegistry {
    config: AppConfig,
    tier_ladder: ResolvedTierLadder,
}

impl CliRegistry {
    /// Create a new CLI registry with the given config
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            tier_ladder: embedded_resolved_tier_ladder(),
        }
    }

    /// Create a registry whose tier ladder is resolved from the explicit
    /// institutional and project roots.
    pub fn new_with_tier_sources(
        config: AppConfig,
        project_path: &Path,
        institutional_wiki_root: Option<&Path>,
    ) -> Self {
        Self {
            config,
            tier_ladder: resolve_tier_ladder(project_path, institutional_wiki_root),
        }
    }

    /// Return the resolved snapshot and its load-time issues.
    pub fn tier_ladder_resolution(&self) -> &ResolvedTierLadder {
        &self.tier_ladder
    }

    /// Get CLI configuration for a specific CLI name
    pub fn get_cli(&self, name: &str) -> Option<&CliConfig> {
        if !crate::adapters::is_valid_cli(name) {
            return None;
        }
        self.config.clis.get(name)
    }

    /// Get all registered CLIs
    pub fn list_clis(&self) -> Vec<String> {
        self.config
            .clis
            .keys()
            .filter(|name| crate::adapters::is_valid_cli(name))
            .cloned()
            .collect()
    }

    /// Build command, arguments, and environment variables from an agent config
    pub fn build_command(&self, agent_config: &AgentConfig) -> Result<BuiltCommand, RegistryError> {
        if !crate::adapters::is_valid_cli(&agent_config.cli) {
            return Err(RegistryError::UnknownCli(agent_config.cli.clone()));
        }
        let cli = self
            .config
            .clis
            .get(&agent_config.cli)
            .ok_or_else(|| RegistryError::UnknownCli(agent_config.cli.clone()))?;
        let (model, extra_flags) = Self::resolve_model_and_flags(
            &agent_config.cli,
            agent_config.model.as_deref(),
            Some(&cli.default_model),
            &agent_config.flags,
        );

        let mut args = Vec::new();
        let mut env = HashMap::new();

        // Add auto-approve flag
        if let Some(ref flag) = cli.auto_approve_flag {
            args.push(flag.clone());
        }

        // Add model flag
        if let Some(ref model_flag) = cli.model_flag {
            if let Some(model) = model {
                args.push(model_flag.clone());
                args.push(model);
            }
        }

        // Add environment variables from CLI config
        if let Some(ref cli_env) = cli.env {
            env.extend(cli_env.clone());
        }

        // Add custom flags from agent config
        args.extend(extra_flags);

        Ok(BuiltCommand {
            command: cli.command.clone(),
            args,
            env,
        })
    }

    /// Build command with additional prompt injection
    pub fn build_command_with_prompt(
        &self,
        agent_config: &AgentConfig,
        prompt: Option<&str>,
    ) -> Result<BuiltCommand, RegistryError> {
        let mut built = self.build_command(agent_config)?;

        // Add prompt flag for CLIs that support it
        if let Some(prompt) = prompt {
            if !prompt.is_empty() {
                match agent_config.cli.as_str() {
                    "claude" => {
                        built.args.push("-p".to_string());
                        built.args.push(prompt.to_string());
                    }
                    // Add other CLIs as needed
                    _ => {
                        // Some CLIs might use different prompt flags
                    }
                }
            }
        }

        Ok(built)
    }

    /// Get default CLI and model for a role type
    pub fn get_role_defaults(&self, role_type: &str) -> Option<(&str, &str)> {
        self.config
            .default_roles
            .get(role_type)
            .filter(|defaults| crate::adapters::is_valid_cli(&defaults.cli))
            .map(|defaults| (defaults.cli.as_str(), defaults.model.as_str()))
    }

    /// Get the built-in default model for a CLI.
    ///
    /// Returns `None` for CLIs whose model is set out-of-band. Frontend uses
    /// `None` as the signal to hide the model field.
    pub fn default_model(cli: &str) -> Option<&'static str> {
        match cli {
            "claude" => Some("opus"),
            "codex" => Some("gpt-6.1-sol"),
            "cursor" => Some("composer-2.5"),
            _ => None,
        }
    }

    /// Normalize known legacy model aliases at the CLI launch boundary.
    ///
    /// Older Hive Manager builds persisted `gpt-5.6`, while Codex sessions
    /// authenticated through a ChatGPT account require the concrete Sol catalog
    /// ID. Retired Cursor Composer IDs resolve to their current equivalents.
    /// Keep arbitrary operator-selected model IDs untouched.
    pub fn normalize_model<'a>(cli: &str, model: &'a str) -> &'a str {
        match (cli, model) {
            ("codex", "gpt-6") => "gpt-6-sol",
            ("codex", "gpt-5.6") => "gpt-5.6-sol",
            ("cursor", "composer-1" | "composer-2") => "composer-2.5",
            ("cursor", "composer-2-fast") => "composer-2.5-fast",
            _ => model,
        }
    }

    /// Resolve the effective model and remove duplicate Codex model flags.
    ///
    /// The typed `model` field is authoritative. For older/manual configs that
    /// only supplied `-m` or `--model` in `flags`, preserve that choice while
    /// emitting a single normalized model argument.
    pub fn resolve_model_and_flags(
        cli: &str,
        configured_model: Option<&str>,
        default_model: Option<&str>,
        flags: &[String],
    ) -> (Option<String>, Vec<String>) {
        if cli != "codex" {
            return (
                configured_model
                    .or(default_model)
                    .map(|model| Self::normalize_model(cli, model).to_string()),
                flags.to_vec(),
            );
        }

        let mut flag_model = None;
        let mut extra_flags = Vec::with_capacity(flags.len());
        let mut index = 0;
        while index < flags.len() {
            let flag = &flags[index];
            if flag == "-m" || flag == "--model" {
                if let Some(model) = flags.get(index + 1).filter(|model| !model.is_empty()) {
                    flag_model = Some(model.as_str());
                    index += 2;
                    continue;
                }
            } else if let Some(model) = flag
                .strip_prefix("--model=")
                .or_else(|| flag.strip_prefix("-m="))
                .filter(|model| !model.is_empty())
            {
                flag_model = Some(model);
                index += 1;
                continue;
            }

            extra_flags.push(flag.clone());
            index += 1;
        }

        let model = configured_model
            .or(flag_model)
            .or(default_model)
            .map(|model| Self::normalize_model(cli, model).to_string());
        (model, extra_flags)
    }

    /// Resolve one task tier from this registry's layered ladder snapshot.
    /// Unsupported providers and an unavailable maintained base return `None`.
    pub fn resolve_tier(&self, provider: &str, tier: TaskTier) -> Option<ResolvedTier> {
        let ladder = self.tier_ladder.ladder.as_ref()?;
        Self::resolve_tier_from_ladder(ladder, provider, tier)
    }

    /// Expand one cell from a previously resolved ladder through the registry's
    /// authoritative eight-preset table.
    pub fn resolve_tier_from_ladder(
        ladder: &TierLadder,
        provider: &str,
        tier: TaskTier,
    ) -> Option<ResolvedTier> {
        let preset_id = ladder.preset_id(provider, tier)?;
        expand_preset(provider, preset_id).map(|preset| preset.resolved)
    }

    /// Infer runtime capability facts for a CLI harness.
    ///
    /// Capability support and operator authorization are deliberately separate:
    /// this method reports only facts the harness integration knows. A custom or
    /// not-yet-profiled CLI remains `Unknown` rather than being treated as either
    /// supported or unsupported.
    pub fn infer_capabilities(cli: &str) -> CapabilityCard {
        CapabilityCard {
            native_delegation: match cli {
                "claude" | "codex" => CapabilitySupport::Supported,
                _ => CapabilitySupport::Unknown,
            },
        }
    }

    /// Resolve whether native delegation is authorized for this launch.
    ///
    /// `Encouraged` is an explicit operator authorization, including for a
    /// capability whose support is not yet known. It never rewrites the card's
    /// factual support value, and a known `Unsupported` capability remains off.
    pub fn native_delegation_authorized(card: &CapabilityCard, policy: &DelegationPolicy) -> bool {
        match policy.mode {
            NativeDelegationMode::Disabled => false,
            NativeDelegationMode::Auto => card.native_delegation == CapabilitySupport::Supported,
            NativeDelegationMode::Encouraged => {
                card.native_delegation != CapabilitySupport::Unsupported
            }
        }
    }

    /// Update the config
    pub fn update_config(&mut self, config: AppConfig) {
        self.config = config;
    }

    /// Get the behavioral profile for a CLI
    pub fn get_behavior(cli: &str) -> CliBehavior {
        match cli {
            "claude" => CliBehavior::ActionProne,
            // Codex principals commonly start in STANDBY and are activated by
            // a task-file update. Until the runtime injects an explicit wake-up,
            // they need a durable activation loop.
            "codex" => CliBehavior::ExplicitPolling,
            "cursor" => CliBehavior::Interactive,
            _ => CliBehavior::ActionProne, // Default to most constrained
        }
    }

    /// Check if a CLI needs role hardening (stronger constraints in prompts)
    pub fn needs_role_hardening(cli: &str) -> bool {
        matches!(Self::get_behavior(cli), CliBehavior::ActionProne)
    }

    /// A role may declare a behavior override; otherwise capability inheritance
    /// preserves the CLI's existing behavior exactly.
    pub fn get_behavior_for_role(cli: &str, definition: Option<&RoleDefinition>) -> CliBehavior {
        definition
            .and_then(|definition| definition.behavior)
            .unwrap_or_else(|| Self::get_behavior(cli))
    }
}

/// A built command ready for execution
#[derive(Debug, Clone)]
pub struct BuiltCommand {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

impl BuiltCommand {
    /// Get command as string slice
    pub fn command(&self) -> &str {
        &self.command
    }

    /// Get args as vector of string slices
    pub fn args_as_str(&self) -> Vec<&str> {
        self.args.iter().map(|s| s.as_str()).collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("Unknown CLI: {0}")]
    UnknownCli(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> AppConfig {
        let mut clis = HashMap::new();
        clis.insert(
            "claude".to_string(),
            CliConfig {
                command: "claude".to_string(),
                auto_approve_flag: Some("--dangerously-skip-permissions".to_string()),
                model_flag: Some("--model".to_string()),
                default_model: "opus".to_string(),
                env: None,
            },
        );
        clis.insert(
            "cursor".to_string(),
            CliConfig {
                command: "wsl".to_string(),
                auto_approve_flag: Some("--force".to_string()),
                model_flag: Some("--model".to_string()),
                default_model: "composer-2.5".to_string(),
                env: None,
            },
        );
        clis.insert(
            "codex".to_string(),
            CliConfig {
                command: "codex".to_string(),
                auto_approve_flag: Some("--dangerously-bypass-approvals-and-sandbox".to_string()),
                model_flag: Some("-m".to_string()),
                default_model: "gpt-6.1-sol".to_string(),
                env: None,
            },
        );

        AppConfig {
            clis,
            default_roles: HashMap::new(),
            api: crate::storage::ApiConfig {
                enabled: true,
                port: 18800,
            },
            global_wiki_path: None,
            knowledge_wiki_folders: None,
            pty_replay_buffer_bytes: None,
            slop_check_mode: None,
            maintain_check_mode: None,
        }
    }

    #[test]
    fn test_build_claude_command() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "claude".to_string(),
            model: Some("sonnet".to_string()),
            flags: vec![],
            label: None,
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "claude");
        assert!(built
            .args
            .contains(&"--dangerously-skip-permissions".to_string()));
        assert!(built.args.contains(&"--model".to_string()));
        assert!(built.args.contains(&"sonnet".to_string()));
    }

    #[test]
    fn test_build_command_with_prompt() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "claude".to_string(),
            model: None,
            flags: vec![],
            label: None,
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry
            .build_command_with_prompt(&config, Some("Test prompt"))
            .unwrap();
        assert!(built.args.contains(&"-p".to_string()));
        assert!(built.args.contains(&"Test prompt".to_string()));
    }

    #[test]
    fn test_build_cursor_command() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "cursor".to_string(),
            model: None,
            flags: vec![],
            label: None,
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "wsl");
        assert!(built.args.contains(&"--force".to_string()));
        assert!(built
            .args
            .windows(2)
            .any(|pair| pair == ["--model".to_string(), "composer-2.5".to_string()]));
    }

    #[test]
    fn test_cursor_legacy_models_are_normalized_at_launch() {
        let registry = CliRegistry::new(test_config());
        for (legacy, current) in [
            ("composer-1", "composer-2.5"),
            ("composer-2", "composer-2.5"),
            ("composer-2-fast", "composer-2.5-fast"),
        ] {
            let config = AgentConfig {
                cli: "cursor".to_string(),
                model: Some(legacy.to_string()),
                flags: vec!["--trust".to_string()],
                ..AgentConfig::default()
            };
            let built = registry.build_command(&config).unwrap();
            assert!(built
                .args
                .windows(2)
                .any(|pair| pair == ["--model", current]));
            assert!(!built.args.iter().any(|arg| arg == legacy));
            assert!(built.args.contains(&"--trust".to_string()));
            assert_eq!(CliRegistry::normalize_model("codex", legacy), legacy);

            let (model, _) =
                CliRegistry::resolve_model_and_flags("cursor", None, Some(legacy), &[]);
            assert_eq!(model.as_deref(), Some(current));
        }
    }

    #[test]
    fn test_build_codex_command() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "codex".to_string(),
            model: None,
            flags: vec![],
            label: None,
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "codex");
        assert!(built
            .args
            .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(built.args.contains(&"-m".to_string()));
        assert!(built.args.contains(&"gpt-6.1-sol".to_string()));
    }

    #[test]
    fn test_cli_behavior_profiles() {
        assert_eq!(
            CliRegistry::get_behavior("claude"),
            CliBehavior::ActionProne
        );
        assert_eq!(
            CliRegistry::get_behavior("codex"),
            CliBehavior::ExplicitPolling
        );
        assert_eq!(
            CliRegistry::get_behavior("cursor"),
            CliBehavior::Interactive
        );
        assert_eq!(
            CliRegistry::get_behavior("unknown-cli"),
            CliBehavior::ActionProne
        );
    }

    #[test]
    fn test_needs_role_hardening() {
        assert!(CliRegistry::needs_role_hardening("claude"));
        assert!(!CliRegistry::needs_role_hardening("codex"));
    }

    #[test]
    fn test_get_behavior_for_evaluator_role() {
        let evaluator = crate::orchestrator::org_graph::evaluator_role_definition();
        assert!(matches!(
            CliRegistry::get_behavior_for_role("claude", Some(&evaluator)),
            CliBehavior::InstructionFollowing
        ));
        let backend = crate::orchestrator::org_graph::RoleDefinition::empty("backend");
        assert!(matches!(
            CliRegistry::get_behavior_for_role("claude", Some(&backend)),
            CliBehavior::ActionProne
        ));
    }

    #[test]
    fn test_default_model_lookup() {
        assert_eq!(CliRegistry::default_model("claude"), Some("opus"));
        assert_eq!(CliRegistry::default_model("codex"), Some("gpt-6.1-sol"));
        assert_eq!(CliRegistry::default_model("cursor"), Some("composer-2.5"));
        assert_eq!(CliRegistry::default_model("unknown"), None);
        // Removed CLIs must no longer resolve a default model.
        assert_eq!(CliRegistry::default_model("gemini"), None);
        assert_eq!(CliRegistry::default_model("antigravity"), None);
    }

    #[test]
    fn test_codex_legacy_sol_alias_is_normalized_at_launch() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "codex".to_string(),
            model: Some("gpt-5.6".to_string()),
            ..AgentConfig::default()
        };

        let built = registry.build_command(&config).unwrap();
        assert!(built
            .args
            .windows(2)
            .any(|pair| pair == ["-m".to_string(), "gpt-5.6-sol".to_string()]));
        assert!(!built.args.iter().any(|arg| arg == "gpt-5.6"));
    }

    #[test]
    fn test_codex_gpt_6_alias_is_normalized_at_launch() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "codex".to_string(),
            model: Some("gpt-6".to_string()),
            ..AgentConfig::default()
        };

        let built = registry.build_command(&config).unwrap();
        assert!(built
            .args
            .windows(2)
            .any(|pair| pair == ["-m".to_string(), "gpt-6-sol".to_string()]));
        assert!(!built.args.iter().any(|arg| arg == "gpt-6"));
    }

    #[test]
    fn test_model_normalization_preserves_operator_selected_models() {
        assert_eq!(
            CliRegistry::normalize_model("codex", "gpt-5.6-sol"),
            "gpt-5.6-sol"
        );
        assert_eq!(
            CliRegistry::normalize_model("codex", "operator-selected-model"),
            "operator-selected-model"
        );
        assert_eq!(CliRegistry::normalize_model("claude", "gpt-5.6"), "gpt-5.6");
        assert_eq!(
            CliRegistry::normalize_model("cursor", "gemini-4-argon-high"),
            "gemini-4-argon-high"
        );
        assert_eq!(
            CliRegistry::normalize_model("cursor", "operator-selected-model"),
            "operator-selected-model"
        );
    }

    #[test]
    fn test_codex_model_flags_are_normalized_without_duplicates() {
        let flags = vec![
            "--full-auto".to_string(),
            "-m".to_string(),
            "gpt-5.6".to_string(),
        ];
        let (model, extra_flags) =
            CliRegistry::resolve_model_and_flags("codex", None, Some("gpt-5.6-sol"), &flags);
        assert_eq!(model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(extra_flags, vec!["--full-auto"]);

        let (model, extra_flags) = CliRegistry::resolve_model_and_flags(
            "codex",
            Some("operator-selected-model"),
            Some("gpt-5.6-sol"),
            &["--model=gpt-5.6".to_string()],
        );
        assert_eq!(model.as_deref(), Some("operator-selected-model"));
        assert!(extra_flags.is_empty());
    }

    #[test]
    fn test_capability_inference_is_conservative() {
        assert_eq!(
            CliRegistry::infer_capabilities("claude").native_delegation,
            CapabilitySupport::Supported
        );
        assert_eq!(
            CliRegistry::infer_capabilities("codex").native_delegation,
            CapabilitySupport::Supported
        );
        assert_eq!(
            CliRegistry::infer_capabilities("custom-harness").native_delegation,
            CapabilitySupport::Unknown
        );
    }

    #[test]
    fn test_native_delegation_policy_preserves_support_facts() {
        let supported = CapabilityCard {
            native_delegation: CapabilitySupport::Supported,
        };
        let unsupported = CapabilityCard {
            native_delegation: CapabilitySupport::Unsupported,
        };
        let unknown = CapabilityCard::default();

        let policy = |mode| DelegationPolicy {
            mode,
            max_children: Some(3),
            max_depth: Some(1),
        };

        assert!(!CliRegistry::native_delegation_authorized(
            &supported,
            &policy(NativeDelegationMode::Disabled)
        ));
        assert!(CliRegistry::native_delegation_authorized(
            &supported,
            &policy(NativeDelegationMode::Auto)
        ));
        assert!(!CliRegistry::native_delegation_authorized(
            &unknown,
            &policy(NativeDelegationMode::Auto)
        ));
        assert!(CliRegistry::native_delegation_authorized(
            &unknown,
            &policy(NativeDelegationMode::Encouraged)
        ));
        assert!(!CliRegistry::native_delegation_authorized(
            &unsupported,
            &policy(NativeDelegationMode::Encouraged)
        ));
        assert_eq!(unknown.native_delegation, CapabilitySupport::Unknown);
    }

    #[test]
    fn test_resolve_tier_uses_provider_native_model_and_effort_flags() {
        let registry = CliRegistry::new(test_config());
        let codex = registry.resolve_tier("codex", TaskTier::High).unwrap();
        assert_eq!(codex.model, "gpt-6.1-sol");
        assert_eq!(codex.flags, vec!["-c", "model_reasoning_effort=\"xhigh\""]);

        let claude = registry.resolve_tier("claude", TaskTier::Low).unwrap();
        assert_eq!(claude.model, "claude-haiku-4-5");
        assert!(claude.flags.is_empty());

        assert!(registry.resolve_tier("droid", TaskTier::Medium).is_none());
    }

    #[test]
    fn test_resolve_tier_uses_the_registrys_layered_project_override() {
        let project = tempfile::tempdir().unwrap();
        let tiers = project.path().join(".ai-docs/tiers");
        std::fs::create_dir_all(&tiers).unwrap();
        std::fs::write(
            tiers.join("ladder.md"),
            r#"---
            {
              "tier_ladder": {
                "codex": { "low": "codex-gpt-5-6-sol-medium" }
              }
            }
            ---
            "#,
        )
        .unwrap();

        let registry = CliRegistry::new_with_tier_sources(test_config(), project.path(), None);

        assert_eq!(
            registry.resolve_tier("codex", TaskTier::Low).unwrap(),
            ResolvedTier {
                model: "gpt-5.6-sol".to_string(),
                flags: vec![
                    "-c".to_string(),
                    "model_reasoning_effort=\"medium\"".to_string()
                ],
            }
        );
        assert_eq!(
            registry
                .resolve_tier("codex", TaskTier::High)
                .unwrap()
                .flags,
            vec!["-c", "model_reasoning_effort=\"xhigh\""]
        );
    }

    #[test]
    fn test_build_command_rejects_removed_clis() {
        let mut stale = test_config();
        for cli in ["gemini", "antigravity", "agy", "droid", "opencode", "qwen"] {
            stale.clis.insert(cli.to_string(), stale.clis["claude"].clone());
            stale.default_roles.insert(cli.to_string(), crate::storage::RoleDefaults {
                cli: cli.to_string(), model: "stale-model".to_string(),
            });
        }
        let registry = CliRegistry::new(stale);
        assert_eq!(registry.list_clis().len(), 3);
        for cli in ["gemini", "antigravity", "agy", "droid", "opencode", "qwen"] {
            assert!(registry.get_cli(cli).is_none());
            assert!(registry.get_role_defaults(cli).is_none());
            assert!(CliRegistry::default_model(cli).is_none());
            let config = AgentConfig {
                cli: cli.to_string(),
                model: None,
                flags: vec![],
                label: None,
                name: None,
                description: None,
                role: None,
                initial_prompt: None,
            };
            assert!(
                matches!(
                    registry.build_command(&config),
                    Err(RegistryError::UnknownCli(_))
                ),
                "{cli} must no longer be buildable"
            );
        }
    }
}
