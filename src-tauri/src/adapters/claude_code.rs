//! Claude Code CLI adapter implementation.

use super::{AgentLaunchSpec, AgentSignal, BootstrapContext, CliAdapter, LaunchCommand};

/// Claude Code CLI adapter.
///
/// Claude CLI-specific behavior:
/// - Auto-approve: `--dangerously-skip-permissions`
/// - Model flag: `--model`
/// - Prompts: positional argument (opens interactive mode)
pub struct ClaudeCodeAdapter;

impl CliAdapter for ClaudeCodeAdapter {
    fn cli_name(&self) -> &'static str {
        "claude"
    }

    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand {
        let mut cmd = LaunchCommand::new("claude", spec.cwd.clone());

        // Add auto-approve flag
        cmd = cmd.arg("--dangerously-skip-permissions");

        // Add model if specified
        if let Some(ref model) = spec.model {
            cmd = cmd.arg("--model").arg(model);
        }

        // Add extra flags from spec
        cmd = cmd.args(spec.flags.iter().cloned());

        // Add environment variables
        cmd = cmd.envs(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));

        // Add prompt (positional for interactive mode)
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

        // Claude-specific completion patterns
        if line_lower.contains("task completed") || line_lower.contains("all tasks completed") {
            return Some(AgentSignal::Completed);
        }

        // Error patterns
        if line_lower.contains("error:") || line_lower.contains("failed:") || line_lower.contains("an error occurred") {
            return Some(AgentSignal::Failed {
                message: line.to_string(),
            });
        }

        // Waiting for input patterns
        if line.contains("?") && (line.contains("[Y/n]") || line.contains("[y/N]") || line.contains("(y/n)")) {
            return Some(AgentSignal::WaitingInput);
        }

        // Tool call detection
        if line_lower.contains("using tool:") || line_lower.contains("calling tool:") || line.contains("tool_call") {
            // Extract tool name if possible
            let tool = extract_tool_name(line).unwrap_or_else(|| "unknown".to_string());
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
            "You are a {} agent in session {}.\n",
            context.role, context.session_id
        );

        if let Some(ref task_file) = context.task_file {
            prompt.push_str(&format!(
                "Read {} and execute the instructions.\n",
                task_file.display()
            ));
        }

        prompt.push_str(&format!(
            "Project path: {}\n",
            context.project_path.display()
        ));

        // Add any additional variables
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
}

/// Extract tool name from a tool call line.
fn extract_tool_name(line: &str) -> Option<String> {
    // Common patterns: "Using tool: tool_name" or "tool_call: tool_name"
    let patterns = ["using tool:", "calling tool:", "tool_call:", "tool:"];
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_spec() -> AgentLaunchSpec {
        AgentLaunchSpec {
            cli: "claude".to_string(),
            model: Some("opus".to_string()),
            flags: vec![],
            cwd: PathBuf::from("/project"),
            env: std::collections::HashMap::new(),
            prompt_file: Some(PathBuf::from("/project/prompt.md")),
            inline_task: None,
            role: "worker".to_string(),
            label: Some("Worker 1".to_string()),
        }
    }

    #[test]
    fn test_build_launch_command() {
        let adapter = ClaudeCodeAdapter;
        let spec = make_spec();
        let cmd = adapter.build_launch_command(&spec);

        assert_eq!(cmd.binary, "claude");
        assert!(cmd.args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(cmd.args.contains(&"--model".to_string()));
        assert!(cmd.args.contains(&"opus".to_string()));
    }

    #[test]
    fn test_detect_completed() {
        let adapter = ClaudeCodeAdapter;

        assert_eq!(
            adapter.detect_status_signal("Task completed successfully"),
            Some(AgentSignal::Completed)
        );
    }

    #[test]
    fn test_detect_failed() {
        let adapter = ClaudeCodeAdapter;

        match adapter.detect_status_signal("Error: Something went wrong") {
            Some(AgentSignal::Failed { message }) => {
                assert!(message.contains("Error"));
            }
            _ => panic!("Expected Failed signal"),
        }
    }

    #[test]
    fn test_detect_waiting_input() {
        let adapter = ClaudeCodeAdapter;

        assert_eq!(
            adapter.detect_status_signal("Continue? [Y/n]"),
            Some(AgentSignal::WaitingInput)
        );
    }

    #[test]
    fn test_detect_tool_call() {
        let adapter = ClaudeCodeAdapter;

        match adapter.detect_status_signal("Using tool: read_file") {
            Some(AgentSignal::ToolCall { tool }) => {
                assert_eq!(tool, "read_file");
            }
            _ => panic!("Expected ToolCall signal"),
        }
    }

    #[test]
    fn test_extract_tool_name() {
        assert_eq!(extract_tool_name("Using tool: read_file"), Some("read_file".to_string()));
        assert_eq!(extract_tool_name("Calling tool: write_file"), Some("write_file".to_string()));
        assert_eq!(extract_tool_name("No tool here"), None);
    }

    #[test]
    fn test_extract_tool_name_handles_unicode_before_marker() {
        assert_eq!(
            extract_tool_name("İ Using tool: read_file"),
            Some("read_file".to_string())
        );
    }
}
