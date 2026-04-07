//! Qwen CLI adapter implementation.

use super::{AgentLaunchSpec, AgentSignal, BootstrapContext, CliAdapter, LaunchCommand};

/// Qwen Code adapter.
///
/// Qwen-specific behavior:
/// - Auto-approve: `-y`
/// - Model flag: `-m`
/// - Prompts: `-i`
pub struct QwenAdapter;

impl CliAdapter for QwenAdapter {
    fn cli_name(&self) -> &'static str {
        "qwen"
    }

    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand {
        let mut cmd = LaunchCommand::new("qwen", spec.cwd.clone()).arg("-y");

        if let Some(ref model) = spec.model {
            cmd = cmd.arg("-m").arg(model);
        }

        cmd = cmd.args(spec.flags.iter().cloned());
        cmd = cmd.envs(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));

        if let Some(ref task) = spec.inline_task {
            cmd = cmd.arg("-i").arg(task);
        } else if let Some(ref prompt_file) = spec.prompt_file {
            let prompt_path = prompt_file.to_string_lossy();
            cmd = cmd.arg("-i").arg(format!("Read {} and execute.", prompt_path));
        }

        cmd
    }

    fn detect_status_signal(&self, line: &str) -> Option<AgentSignal> {
        let line_lower = line.to_lowercase();

        if line_lower.contains("task completed") || line_lower.contains("finished") || line_lower.contains("done") {
            return Some(AgentSignal::Completed);
        }

        if line_lower.contains("error:") || line_lower.contains("failed") || line_lower.contains("exception") {
            return Some(AgentSignal::Failed {
                message: line.to_string(),
            });
        }

        if line.contains('?') || line_lower.contains("please provide") {
            return Some(AgentSignal::WaitingInput);
        }

        if line_lower.contains("using tool:") || line_lower.contains("calling tool:") || line_lower.contains("running:") {
            let tool = line
                .split(':')
                .nth(1)
                .map(|value| value.trim().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            return Some(AgentSignal::ToolCall { tool });
        }

        if line_lower.contains("thinking") || line_lower.contains("processing") {
            return Some(AgentSignal::Processing);
        }

        None
    }

    fn build_bootstrap_prompt(&self, context: &BootstrapContext) -> String {
        let mut prompt = format!(
            "You are a {} agent (Qwen) in session {}.\n",
            context.role, context.session_id
        );

        if let Some(ref task_file) = context.task_file {
            prompt.push_str(&format!("Task file: {}\n", task_file.display()));
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
        Some("-y")
    }

    fn model_flag(&self) -> Option<&'static str> {
        Some("-m")
    }

    fn prompt_flag(&self) -> Option<&'static str> {
        Some("-i")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_spec() -> AgentLaunchSpec {
        AgentLaunchSpec {
            cli: "qwen".to_string(),
            model: Some("qwen3-coder".to_string()),
            flags: vec![],
            cwd: PathBuf::from("/project"),
            env: std::collections::HashMap::new(),
            prompt_file: None,
            inline_task: Some("Read the task file".to_string()),
            role: "worker".to_string(),
            label: Some("Worker 1".to_string()),
        }
    }

    #[test]
    fn test_build_launch_command() {
        let adapter = QwenAdapter;
        let spec = make_spec();
        let cmd = adapter.build_launch_command(&spec);

        assert_eq!(cmd.binary, "qwen");
        assert!(cmd.args.contains(&"-y".to_string()));
        assert!(cmd.args.contains(&"-m".to_string()));
        assert!(cmd.args.contains(&"qwen3-coder".to_string()));
        assert!(cmd.args.contains(&"-i".to_string()));
        assert!(cmd.args.contains(&"Read the task file".to_string()));
    }

    #[test]
    fn test_cli_name() {
        let adapter = QwenAdapter;
        assert_eq!(adapter.cli_name(), "qwen");
    }
}
