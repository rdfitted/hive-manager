use std::collections::HashMap;

use crate::domain::{CapabilityCard, CapabilitySupport, DelegationPolicy, NativeDelegationMode};
use crate::pty::AgentConfig;
use crate::storage::{AppConfig, CliConfig};

/// CLI behavioral profiles for characterizing how different CLI tools behave
#[derive(Debug, Clone, PartialEq)]
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
}

impl CliRegistry {
    /// Create a new CLI registry with the given config
    pub fn new(config: AppConfig) -> Self {
        Self { config }
    }

    /// Get CLI configuration for a specific CLI name
    pub fn get_cli(&self, name: &str) -> Option<&CliConfig> {
        self.config.clis.get(name)
    }

    /// Get all registered CLIs
    pub fn list_clis(&self) -> Vec<String> {
        self.config.clis.keys().cloned().collect()
    }

    /// Build command, arguments, and environment variables from an agent config
    pub fn build_command(&self, agent_config: &AgentConfig) -> Result<BuiltCommand, RegistryError> {
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
            .map(|defaults| (defaults.cli.as_str(), defaults.model.as_str()))
    }

    /// Get the built-in default model for a CLI.
    ///
    /// Returns `None` for CLIs whose model is set out-of-band (e.g. `antigravity`,
    /// whose model lives in `~/.gemini/antigravity-cli/settings.json`). Frontend
    /// uses `None` as the signal to hide the model field.
    pub fn default_model(cli: &str) -> Option<&'static str> {
        match cli {
            "claude" => Some("opus"),
            "gemini" => Some("gemini-2.5-pro"),
            // antigravity (agy) has no model flag; settings.json owns the model.
            "antigravity" => None,
            "opencode" => Some("opencode/big-pickle"),
            "codex" => Some("gpt-5.6-sol"),
            "cursor" => Some("composer-2.5"),
            "droid" => Some("glm-5.1"),
            "qwen" => Some("qwen3-coder"),
            _ => None,
        }
    }

    /// Normalize known legacy model aliases at the CLI launch boundary.
    ///
    /// Older Hive Manager builds persisted `gpt-5.6`, while Codex sessions
    /// authenticated through a ChatGPT account require the concrete Sol catalog
    /// ID. Keep arbitrary operator-selected model IDs untouched.
    pub fn normalize_model<'a>(cli: &str, model: &'a str) -> &'a str {
        match (cli, model) {
            ("codex", "gpt-5.6") => "gpt-5.6-sol",
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
                configured_model.or(default_model).map(ToString::to_string),
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
            "claude" | "antigravity" | "gemini" => CliBehavior::ActionProne,
            "qwen" => CliBehavior::InstructionFollowing,
            // Codex principals commonly start in STANDBY and are activated by
            // a task-file update. Until the runtime injects an explicit wake-up,
            // they need the same durable activation loop as OpenCode.
            "codex" | "opencode" => CliBehavior::ExplicitPolling,
            "droid" | "cursor" => CliBehavior::Interactive,
            _ => CliBehavior::ActionProne, // Default to most constrained
        }
    }

    /// Check if a CLI needs role hardening (stronger constraints in prompts)
    pub fn needs_role_hardening(cli: &str) -> bool {
        matches!(Self::get_behavior(cli), CliBehavior::ActionProne)
    }

    /// Evaluators should default to a skeptical, instruction-following profile
    /// even when the underlying CLI is more action-prone in other roles.
    pub fn get_behavior_for_role(cli: &str, role_type: Option<&str>) -> CliBehavior {
        match role_type.map(str::to_ascii_lowercase).as_deref() {
            Some("evaluator") => CliBehavior::InstructionFollowing,
            _ => Self::get_behavior(cli),
        }
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
            "gemini".to_string(),
            CliConfig {
                command: "gemini".to_string(),
                auto_approve_flag: Some("-y".to_string()),
                model_flag: Some("-m".to_string()),
                default_model: "gemini-2.5-pro".to_string(),
                env: None,
            },
        );
        clis.insert(
            "antigravity".to_string(),
            CliConfig {
                command: "agy".to_string(),
                auto_approve_flag: Some("--dangerously-skip-permissions".to_string()),
                // agy has no model flag — model lives in ~/.gemini/antigravity-cli/settings.json
                model_flag: None,
                default_model: String::new(),
                env: None,
            },
        );
        clis.insert(
            "cursor".to_string(),
            CliConfig {
                command: "wsl".to_string(),
                auto_approve_flag: Some("--force".to_string()),
                model_flag: None, // Cursor uses global model setting
                default_model: "composer-2.5".to_string(),
                env: None,
            },
        );
        clis.insert(
            "droid".to_string(),
            CliConfig {
                command: "droid".to_string(),
                auto_approve_flag: None, // Interactive mode - no auto-approve flag
                model_flag: None,        // Model selected via /model command in TUI
                default_model: "glm-5.1".to_string(),
                env: None,
            },
        );
        clis.insert(
            "qwen".to_string(),
            CliConfig {
                command: "qwen".to_string(),
                auto_approve_flag: Some("-y".to_string()),
                model_flag: Some("-m".to_string()),
                default_model: "qwen3-coder".to_string(),
                env: None,
            },
        );
        clis.insert(
            "codex".to_string(),
            CliConfig {
                command: "codex".to_string(),
                auto_approve_flag: Some("--dangerously-bypass-approvals-and-sandbox".to_string()),
                model_flag: Some("-m".to_string()),
                default_model: "gpt-5.6-sol".to_string(),
                env: None,
            },
        );
        clis.insert(
            "opencode".to_string(),
            CliConfig {
                command: "opencode".to_string(),
                auto_approve_flag: None,
                model_flag: Some("-m".to_string()),
                default_model: "opencode/big-pickle".to_string(),
                env: Some({
                    let mut env = HashMap::new();
                    env.insert("OPENCODE_YOLO".to_string(), "true".to_string());
                    env
                }),
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
    }

    #[test]
    fn test_build_droid_command() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "droid".to_string(),
            model: Some("glm-5.1".to_string()),
            flags: vec![],
            label: None,
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "droid");
        // Droid interactive mode - no auto-approve or model flags
        // Model is selected via /model command in TUI
        assert!(built.args.is_empty() || built.args == config.flags);
    }

    #[test]
    fn test_build_qwen_command() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "qwen".to_string(),
            model: None,
            flags: vec![],
            label: None,
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "qwen");
        assert!(built.args.contains(&"-y".to_string()));
        assert!(built.args.contains(&"-m".to_string()));
        assert!(built.args.contains(&"qwen3-coder".to_string()));
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
        assert!(built.args.contains(&"gpt-5.6-sol".to_string()));
    }

    #[test]
    fn test_build_opencode_command() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "opencode".to_string(),
            model: None,
            flags: vec![],
            label: None,
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "opencode");
        assert!(built.args.contains(&"-m".to_string()));
        assert!(built.args.contains(&"opencode/big-pickle".to_string()));
        // Check env has OPENCODE_YOLO
        assert_eq!(built.env.get("OPENCODE_YOLO"), Some(&"true".to_string()));
    }

    #[test]
    fn test_cli_behavior_profiles() {
        assert_eq!(
            CliRegistry::get_behavior("claude"),
            CliBehavior::ActionProne
        );
        assert_eq!(
            CliRegistry::get_behavior("gemini"),
            CliBehavior::ActionProne
        );
        assert_eq!(
            CliRegistry::get_behavior("antigravity"),
            CliBehavior::ActionProne
        );
        assert_eq!(
            CliRegistry::get_behavior("qwen"),
            CliBehavior::InstructionFollowing
        );
        assert_eq!(
            CliRegistry::get_behavior("codex"),
            CliBehavior::ExplicitPolling
        );
        assert_eq!(
            CliRegistry::get_behavior("opencode"),
            CliBehavior::ExplicitPolling
        );
        assert_eq!(CliRegistry::get_behavior("droid"), CliBehavior::Interactive);
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
        assert!(CliRegistry::needs_role_hardening("gemini"));
        assert!(CliRegistry::needs_role_hardening("antigravity"));
        assert!(!CliRegistry::needs_role_hardening("qwen"));
        assert!(!CliRegistry::needs_role_hardening("codex"));
        assert!(!CliRegistry::needs_role_hardening("droid"));
    }

    #[test]
    fn test_get_behavior_for_evaluator_role() {
        assert!(matches!(
            CliRegistry::get_behavior_for_role("claude", Some("evaluator")),
            CliBehavior::InstructionFollowing
        ));
        assert!(matches!(
            CliRegistry::get_behavior_for_role("claude", Some("backend")),
            CliBehavior::ActionProne
        ));
    }

    #[test]
    fn test_build_antigravity_command() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "antigravity".to_string(),
            // Model is intentionally provided; antigravity must ignore it because
            // agy has no --model flag.
            model: Some("Gemini 3.1 Pro".to_string()),
            flags: vec![],
            label: None,
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "agy");
        assert!(built
            .args
            .contains(&"--dangerously-skip-permissions".to_string()));
        assert!(
            !built.args.iter().any(|a| a == "-m" || a == "--model"),
            "antigravity must not produce a model flag (model lives in settings.json)"
        );
        assert!(
            !built.args.iter().any(|a| a.contains("Gemini 3.1 Pro")),
            "Model value from config must not leak into args"
        );
    }

    #[test]
    fn test_default_model_lookup() {
        assert_eq!(CliRegistry::default_model("claude"), Some("opus"));
        assert_eq!(CliRegistry::default_model("gemini"), Some("gemini-2.5-pro"));
        assert_eq!(CliRegistry::default_model("codex"), Some("gpt-5.6-sol"));
        assert_eq!(CliRegistry::default_model("droid"), Some("glm-5.1"));
        assert_eq!(CliRegistry::default_model("cursor"), Some("composer-2.5"));
        assert_eq!(CliRegistry::default_model("unknown"), None);
        // antigravity has no model flag — None signals the UI to hide the field.
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
    fn test_model_normalization_preserves_operator_selected_models() {
        assert_eq!(
            CliRegistry::normalize_model("codex", "operator-selected-model"),
            "operator-selected-model"
        );
        assert_eq!(CliRegistry::normalize_model("claude", "gpt-5.6"), "gpt-5.6");
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
        assert_eq!(
            CliRegistry::infer_capabilities("antigravity").native_delegation,
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
    fn test_build_gemini_command() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "gemini".to_string(),
            model: Some("gemini-2.5-pro".to_string()),
            flags: vec![],
            label: None,
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "gemini");
        assert!(built.args.contains(&"-y".to_string()));
        assert!(built.args.contains(&"-m".to_string()));
        assert!(built.args.contains(&"gemini-2.5-pro".to_string()));
    }
}
