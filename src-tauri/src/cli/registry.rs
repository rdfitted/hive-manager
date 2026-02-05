use std::collections::HashMap;

use crate::pty::AgentConfig;
use crate::storage::{AppConfig, CliConfig};

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
    pub fn build_command(
        &self,
        agent_config: &AgentConfig,
    ) -> Result<BuiltCommand, RegistryError> {
        let cli = self.config.clis.get(&agent_config.cli)
            .ok_or_else(|| RegistryError::UnknownCli(agent_config.cli.clone()))?;

        let mut args = Vec::new();
        let mut env = HashMap::new();

        // Add auto-approve flag
        if let Some(ref flag) = cli.auto_approve_flag {
            args.push(flag.clone());
        }

        // Add model flag
        if let Some(ref model_flag) = cli.model_flag {
            let model = agent_config.model.as_ref()
                .unwrap_or(&cli.default_model);
            args.push(model_flag.clone());
            args.push(model.clone());
        }

        // Add environment variables from CLI config
        if let Some(ref cli_env) = cli.env {
            env.extend(cli_env.clone());
        }

        // Add custom flags from agent config
        args.extend(agent_config.flags.clone());

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
        self.config.default_roles.get(role_type)
            .map(|defaults| (defaults.cli.as_str(), defaults.model.as_str()))
    }

    /// Update the config
    pub fn update_config(&mut self, config: AppConfig) {
        self.config = config;
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
        clis.insert("claude".to_string(), CliConfig {
            command: "claude".to_string(),
            auto_approve_flag: Some("--dangerously-skip-permissions".to_string()),
            model_flag: Some("--model".to_string()),
            default_model: "opus".to_string(),
            env: None,
        });
        clis.insert("gemini".to_string(), CliConfig {
            command: "gemini".to_string(),
            auto_approve_flag: Some("-y".to_string()),
            model_flag: Some("-m".to_string()),
            default_model: "gemini-2.5-pro".to_string(),
            env: None,
        });
        clis.insert("cursor".to_string(), CliConfig {
            command: "wsl".to_string(),
            auto_approve_flag: Some("--force".to_string()),
            model_flag: None,
            default_model: "opus-4.5".to_string(),
            env: None,
        });
        clis.insert("droid".to_string(), CliConfig {
            command: "droid".to_string(),
            auto_approve_flag: None,  // Interactive mode uses OAuth
            model_flag: None,  // Model selected via /model command or config
            default_model: "glm-4.7".to_string(),
            env: None,
        });
        clis.insert("qwen".to_string(), CliConfig {
            command: "qwen".to_string(),
            auto_approve_flag: Some("-y".to_string()),
            model_flag: Some("-m".to_string()),
            default_model: "qwen3-coder".to_string(),
            env: None,
        });

        AppConfig {
            clis,
            default_roles: HashMap::new(),
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
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "claude");
        assert!(built.args.contains(&"--dangerously-skip-permissions".to_string()));
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
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command_with_prompt(&config, Some("Test prompt")).unwrap();
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
            model: Some("glm-4.7".to_string()),
            flags: vec![],
            label: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "droid");
        // Droid interactive mode uses OAuth - no auto-approve flag needed
        // Model is selected via /model command or ~/.factory/config.json
    }

    #[test]
    fn test_build_qwen_command() {
        let registry = CliRegistry::new(test_config());
        let config = AgentConfig {
            cli: "qwen".to_string(),
            model: None,
            flags: vec![],
            label: None,
            role: None,
            initial_prompt: None,
        };

        let built = registry.build_command(&config).unwrap();
        assert_eq!(built.command, "qwen");
        assert!(built.args.contains(&"-y".to_string()));
        assert!(built.args.contains(&"-m".to_string()));
        assert!(built.args.contains(&"qwen3-coder".to_string()));
    }
}
