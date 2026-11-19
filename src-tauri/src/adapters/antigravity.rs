//! Google Antigravity CLI (`agy`) adapter.
//!
//! Antigravity CLI-specific behavior (verified against `agy 1.5.12`):
//! - Binary: `agy`
//! - Auto-approve: `--dangerously-skip-permissions`
//! - Model flag: `--model`, taking catalogue IDs such as `gemini-4-neon-high`
//!   (`agy models` lists them; the effort level is part of the ID). Print mode
//!   hard-fails on an unknown model instead of silently falling back.
//! - Initial interactive prompt: `-i` (alias for `--prompt-interactive`)
//! - Non-interactive single-shot: `-p` (alias for `--print`)
//!
//! The adapter was removed in July 2026 when `agy` had no per-invocation model
//! flag; it returned once `--model` became reliable for interactive and
//! headless runs.

use crate::cli::CliRegistry;

use super::{AgentLaunchSpec, AgentSignal, BootstrapContext, CliAdapter, LaunchCommand};

/// Antigravity CLI (`agy`) adapter.
pub struct AntigravityAdapter;

impl CliAdapter for AntigravityAdapter {
    fn cli_name(&self) -> &'static str {
        "antigravity"
    }

    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand {
        let mut cmd = LaunchCommand::new("agy", spec.cwd.clone()).arg("--dangerously-skip-permissions");

        let (model, flags) = CliRegistry::resolve_model_and_flags(
            "antigravity",
            spec.model.as_deref(),
            CliRegistry::default_model("antigravity"),
            &spec.flags,
        );
        if let Some(model) = model {
            cmd = cmd.arg("--model").arg(model);
        }
        cmd = cmd.args(flags);
        cmd = cmd.envs(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));

        // inline_task is solo/single-shot mode: `-p` runs one prompt and exits.
        // prompt_file is worker mode where the agent stays alive: `-i` starts
        // the interactive session with the initial prompt.
        if let Some(ref task) = spec.inline_task {
            cmd = cmd.arg("-p").arg(task);
        } else if let Some(ref prompt_file) = spec.prompt_file {
            let prompt_path = prompt_file.to_string_lossy();
            cmd = cmd.arg("-i").arg(format!("Read {} and execute.", prompt_path));
        }

        cmd
    }

    fn detect_status_signal(&self, line: &str) -> Option<AgentSignal> {
        let line_lower = line.to_lowercase();
        let trimmed = line.trim_end();

        if line_lower.contains("task completed") || line_lower.contains("finished") || line_lower.contains("done!") {
            return Some(AgentSignal::Completed);
        }

        if line_lower.contains("error:") || line_lower.contains("failed") || line_lower.contains("exception") {
            return Some(AgentSignal::Failed {
                message: line.to_string(),
            });
        }

        if is_explicit_prompt_marker(&line_lower, trimmed) {
            return Some(AgentSignal::WaitingInput);
        }

        if line_lower.contains("calling function:") || line_lower.contains("using tool:") {
            let tool = extract_tool_name(line).unwrap_or_else(|| "unknown".to_string());
            return Some(AgentSignal::ToolCall { tool });
        }

        if line_lower.contains("generating") || line_lower.contains("thinking") {
            return Some(AgentSignal::Processing);
        }

        None
    }

    fn build_bootstrap_prompt(&self, context: &BootstrapContext) -> String {
        let mut prompt = format!(
            "You are a {} agent (Antigravity CLI) in session {}.\n",
            context.role, context.session_id
        );

        if let Some(ref task_file) = context.task_file {
            prompt.push_str(&format!("Task file: {}\n", task_file.display()));
        }

        prompt.push_str(&format!(
            "Project directory: {}\n",
            context.project_path.display()
        ));

        for (key, value) in &context.variables {
            prompt.push_str(&format!("{}: {}\n", key, value));
        }

        prompt
    }

    fn auto_approve_flag(&self) -> Option<&'static str> {
        Some("--dangerously-skip-permissions")
    }

    fn model_flag(&self) -> Option<&'static str> {
        Some("--model")
    }

    fn prompt_flag(&self) -> Option<&'static str> {
        Some("-i")
    }
}

fn extract_tool_name(line: &str) -> Option<String> {
    let patterns = ["calling function:", "using tool:", "function:"];
    let line_lower = line.to_lowercase();

    for pattern in patterns.iter() {
        if let Some(pos) = line_lower.find(pattern) {
            let rest = &line_lower[pos + pattern.len()..];
            let tool = rest.split_whitespace().next()?.trim_matches(':').to_string();
            if !tool.is_empty() {
                return Some(tool);
            }
        }
    }
    None
}

fn is_explicit_prompt_marker(line_lower: &str, trimmed: &str) -> bool {
    line_lower.starts_with("input:")
        || line_lower.starts_with("prompt:")
        || trimmed == ">"
        || trimmed == ">>>"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_spec() -> AgentLaunchSpec {
        AgentLaunchSpec {
            cli: "antigravity".to_string(),
            model: None,
            flags: vec![],
            cwd: PathBuf::from("/project"),
            env: std::collections::HashMap::new(),
            prompt_file: Some(PathBuf::from("/project/task.md")),
            inline_task: None,
            role: "worker".to_string(),
            label: Some("Worker 1".to_string()),
        }
    }

    #[test]
    fn worker_launch_uses_agy_with_default_model_and_interactive_prompt() {
        let cmd = AntigravityAdapter.build_launch_command(&make_spec());

        assert_eq!(cmd.binary, "agy");
        assert_eq!(cmd.args[0], "--dangerously-skip-permissions");
        assert!(cmd.args.windows(2).any(|pair| pair == ["--model", "gemini-4-neon-high"]));
        let prompt_at = cmd.args.iter().position(|arg| arg == "-i").unwrap();
        assert_eq!(cmd.args[prompt_at + 1], "Read /project/task.md and execute.");
        assert!(!cmd.args.contains(&"-p".to_string()));
    }

    #[test]
    fn configured_model_is_forwarded_once() {
        let mut spec = make_spec();
        spec.model = Some("gemini-4-hydrogen-xhigh".to_string());
        let cmd = AntigravityAdapter.build_launch_command(&spec);

        assert!(cmd.args.windows(2).any(|pair| pair == ["--model", "gemini-4-hydrogen-xhigh"]));
        assert_eq!(cmd.args.iter().filter(|arg| *arg == "--model").count(), 1);
    }

    #[test]
    fn inline_task_uses_print_mode() {
        let mut spec = make_spec();
        spec.prompt_file = None;
        spec.inline_task = Some("Fix the bug".to_string());
        let cmd = AntigravityAdapter.build_launch_command(&spec);

        let print_at = cmd.args.iter().position(|arg| arg == "-p").unwrap();
        assert_eq!(cmd.args[print_at + 1], "Fix the bug");
        assert!(!cmd.args.contains(&"-i".to_string()));
    }

    #[test]
    fn detects_status_signals() {
        let adapter = AntigravityAdapter;
        assert_eq!(adapter.detect_status_signal("Task completed"), Some(AgentSignal::Completed));
        assert!(matches!(adapter.detect_status_signal("Error: boom"), Some(AgentSignal::Failed { .. })));
        assert_eq!(adapter.detect_status_signal(">"), Some(AgentSignal::WaitingInput));
        assert_eq!(
            adapter.detect_status_signal("Calling function: read_file"),
            Some(AgentSignal::ToolCall { tool: "read_file".to_string() })
        );
        assert_eq!(adapter.detect_status_signal("Thinking..."), Some(AgentSignal::Processing));
        assert_eq!(adapter.detect_status_signal("plain output"), None);
    }

    #[test]
    fn exposes_flags() {
        let adapter = AntigravityAdapter;
        assert_eq!(adapter.cli_name(), "antigravity");
        assert_eq!(adapter.auto_approve_flag(), Some("--dangerously-skip-permissions"));
        assert_eq!(adapter.model_flag(), Some("--model"));
        assert_eq!(adapter.prompt_flag(), Some("-i"));
    }
}
