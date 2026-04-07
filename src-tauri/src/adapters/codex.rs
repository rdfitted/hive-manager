//! Codex CLI adapter implementation.

use super::{AgentLaunchSpec, AgentSignal, BootstrapContext, CliAdapter, LaunchCommand};

/// Codex CLI adapter.
///
/// Codex CLI-specific behavior:
/// - Auto-approve: `--dangerously-bypass-approvals-and-sandbox`
/// - Model flag: `-m`
/// - Prompts: positional argument
pub struct CodexAdapter;

impl CliAdapter for CodexAdapter {
    fn cli_name(&self) -> &'static str {
        "codex"
    }

    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand {
        let mut cmd = LaunchCommand::new("codex", spec.cwd.clone());

        // Add auto-approve flag (Codex uses a very long flag)
        cmd = cmd.arg("--dangerously-bypass-approvals-and-sandbox");

        // Add model if specified
        if let Some(ref model) = spec.model {
            cmd = cmd.arg("-m").arg(model);
        }

        // Add extra flags from spec
        cmd = cmd.args(spec.flags.iter().cloned());

        // Add environment variables
        cmd = cmd.envs(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));

        // Add prompt (positional for Codex)
        if let Some(ref task) = spec.inline_task {
            cmd = cmd.arg(task);
        } else if let Some(ref prompt_file) = spec.prompt_file {
            let prompt_path = prompt_file.to_string_lossy();
            cmd = cmd.arg(format!("Read {} and execute.", prompt_path));
        }

        cmd
    }

    fn detect_status_signal(&self, line: &str) -> Option<AgentSignal> {
        let line_lower = line.to_lowercase();

        // Codex completion patterns
        if line_lower.contains("task completed") || line_lower.contains("finished") || line_lower.contains("done.") {
            return Some(AgentSignal::Completed);
        }

        // Error patterns
        if line_lower.contains("error:") || line_lower.contains("failed") || line_lower.contains("exception") {
            return Some(AgentSignal::Failed {
                message: line.to_string(),
            });
        }

        // Waiting for input patterns
        if line.contains("?") && line.matches('?').count() <= 2 {
            // Simple heuristic for prompt questions
            return Some(AgentSignal::WaitingInput);
        }

        // Tool call detection (Codex format)
        if line_lower.contains("running:") || line_lower.contains("executing:") {
            let tool = line.split(':').nth(1).map(|s| s.trim().to_string()).unwrap_or_else(|| "unknown".to_string());
            return Some(AgentSignal::ToolCall { tool });
        }

        // Processing indicator
        if line_lower.contains("thinking") || line_lower.contains("processing") {
            return Some(AgentSignal::Processing);
        }

        None
    }

    fn build_bootstrap_prompt(&self, context: &BootstrapContext) -> String {
        let mut prompt = format!(
            "You are a {} agent (Codex) in session {}.\n",
            context.role, context.session_id
        );

        if let Some(ref task_file) = context.task_file {
            prompt.push_str(&format!(
                "Task file: {}\n",
                task_file.display()
            ));
        }

        prompt.push_str(&format!(
            "Working directory: {}\n",
            context.project_path.display()
        ));

        for (key, value) in &context.variables {
            prompt.push_str(&format!("{}: {}\n", key, value));
        }

        prompt
    }

    fn auto_approve_flag(&self) -> Option<&'static str> {
        Some("--dangerously-bypass-approvals-and-sandbox")
    }

    fn model_flag(&self) -> Option<&'static str> {
        Some("-m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_spec() -> AgentLaunchSpec {
        AgentLaunchSpec {
            cli: "codex".to_string(),
            model: Some("o4-mini".to_string()),
            flags: vec![],
            cwd: PathBuf::from("/project"),
            env: std::collections::HashMap::new(),
            prompt_file: None,
            inline_task: Some("Fix the bug".to_string()),
            role: "worker".to_string(),
            label: Some("Worker 1".to_string()),
        }
    }

    #[test]
    fn test_build_launch_command() {
        let adapter = CodexAdapter;
        let spec = make_spec();
        let cmd = adapter.build_launch_command(&spec);

        assert_eq!(cmd.binary, "codex");
        assert!(cmd.args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(cmd.args.contains(&"-m".to_string()));
        assert!(cmd.args.contains(&"o4-mini".to_string()));
        assert!(cmd.args.contains(&"Fix the bug".to_string()));
    }

    #[test]
    fn test_detect_completed() {
        let adapter = CodexAdapter;

        assert_eq!(
            adapter.detect_status_signal("Task completed successfully"),
            Some(AgentSignal::Completed)
        );

        assert_eq!(
            adapter.detect_status_signal("Done."),
            Some(AgentSignal::Completed)
        );
    }

    #[test]
    fn test_detect_failed() {
        let adapter = CodexAdapter;

        match adapter.detect_status_signal("Error: Build failed") {
            Some(AgentSignal::Failed { message }) => {
                assert!(message.contains("Error"));
            }
            _ => panic!("Expected Failed signal"),
        }
    }

    #[test]
    fn test_cli_name() {
        let adapter = CodexAdapter;
        assert_eq!(adapter.cli_name(), "codex");
    }
}
