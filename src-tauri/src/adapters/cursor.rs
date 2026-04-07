//! Cursor CLI adapter implementation.

use super::{AgentLaunchSpec, AgentSignal, BootstrapContext, CliAdapter, LaunchCommand};

/// Cursor Agent adapter.
///
/// Cursor-specific behavior:
/// - Binary: `wsl`
/// - Base args: `-d Ubuntu /root/.local/bin/agent --force`
/// - Model: uses Cursor global configuration, no CLI flag
/// - Prompts: positional argument after the WSL agent command
pub struct CursorAdapter;

impl CliAdapter for CursorAdapter {
    fn cli_name(&self) -> &'static str {
        "cursor"
    }

    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand {
        let mut cmd = LaunchCommand::new("wsl", spec.cwd.clone())
            .arg("-d")
            .arg("Ubuntu")
            .arg("/root/.local/bin/agent")
            .arg("--force");

        cmd = cmd.args(spec.flags.iter().cloned());
        cmd = cmd.envs(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));

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

        if line_lower.contains("task completed") || line_lower.contains("finished") || line_lower.contains("done.") {
            return Some(AgentSignal::Completed);
        }

        if line_lower.contains("error:") || line_lower.contains("failed") || line_lower.contains("exception") {
            return Some(AgentSignal::Failed {
                message: line.to_string(),
            });
        }

        if line.contains('?') && (line.contains("[Y/n]") || line.contains("[y/N]") || line.contains("(y/n)")) {
            return Some(AgentSignal::WaitingInput);
        }

        if line_lower.contains("running:") || line_lower.contains("executing:") || line_lower.contains("using tool:") {
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
            "You are a {} agent (Cursor) in session {}.\n",
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_spec() -> AgentLaunchSpec {
        AgentLaunchSpec {
            cli: "cursor".to_string(),
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
    fn test_build_launch_command() {
        let adapter = CursorAdapter;
        let spec = make_spec();
        let cmd = adapter.build_launch_command(&spec);

        assert_eq!(cmd.binary, "wsl");
        assert_eq!(cmd.args[0..4], ["-d", "Ubuntu", "/root/.local/bin/agent", "--force"]);
        assert!(cmd.args.iter().any(|arg| arg.contains("Read /project/task.md and execute.")));
    }

    #[test]
    fn test_cli_name() {
        let adapter = CursorAdapter;
        assert_eq!(adapter.cli_name(), "cursor");
    }
}
