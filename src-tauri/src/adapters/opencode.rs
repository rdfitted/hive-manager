//! OpenCode CLI adapter implementation.

use super::{AgentLaunchSpec, AgentSignal, BootstrapContext, CliAdapter, LaunchCommand};

/// OpenCode CLI adapter.
///
/// OpenCode CLI-specific behavior:
/// - Auto-approve: Uses `OPENCODE_YOLO=true` environment variable
/// - Model flag: `-m`
/// - Prompts: `--prompt` flag
pub struct OpenCodeAdapter;

impl CliAdapter for OpenCodeAdapter {
    fn cli_name(&self) -> &'static str {
        "opencode"
    }

    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand {
        let mut cmd = LaunchCommand::new("opencode", spec.cwd.clone());

        // OpenCode uses OPENCODE_YOLO=true env var for auto-approve
        cmd = cmd.env("OPENCODE_YOLO", "true");

        // Add model if specified
        if let Some(ref model) = spec.model {
            cmd = cmd.arg("-m").arg(model);
        }

        // Add extra flags from spec
        cmd = cmd.args(spec.flags.iter().cloned());

        // Add environment variables (merge with OPENCODE_YOLO)
        cmd = cmd.envs(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));

        // Add prompt using --prompt flag
        if let Some(ref task) = spec.inline_task {
            cmd = cmd.arg("--prompt").arg(task);
        } else if let Some(ref prompt_file) = spec.prompt_file {
            let prompt_path = prompt_file.to_string_lossy();
            cmd = cmd.arg("--prompt").arg(format!("Read {} and execute.", prompt_path));
        }

        cmd
    }

    fn detect_status_signal(&self, line: &str) -> Option<AgentSignal> {
        let line_lower = line.to_lowercase();

        // OpenCode completion patterns
        if line_lower.contains("task completed") || line_lower.contains("finished") || line_lower.contains("done") {
            return Some(AgentSignal::Completed);
        }

        // Error patterns
        if line_lower.contains("error:") || line_lower.contains("failed") || line_lower.contains("exception") {
            return Some(AgentSignal::Failed {
                message: line.to_string(),
            });
        }

        // Waiting for input patterns
        if line.contains("?") || line_lower.contains("waiting for input") {
            return Some(AgentSignal::WaitingInput);
        }

        // Tool call detection (OpenCode format)
        if line_lower.contains("executing:") || line_lower.contains("running command:") {
            let tool = extract_opencode_tool(line).unwrap_or_else(|| "unknown".to_string());
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
            "You are a {} agent (OpenCode) in session {}.\n",
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

    // OpenCode uses env var, not a flag
    fn auto_approve_flag(&self) -> Option<&'static str> {
        None
    }

    fn model_flag(&self) -> Option<&'static str> {
        Some("-m")
    }

    fn prompt_flag(&self) -> Option<&'static str> {
        Some("--prompt")
    }
}

/// Extract tool/command name from OpenCode output.
fn extract_opencode_tool(line: &str) -> Option<String> {
    let patterns = ["executing:", "running command:"];
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
            cli: "opencode".to_string(),
            model: Some("claude-3.5-sonnet".to_string()),
            flags: vec![],
            cwd: PathBuf::from("/project"),
            env: std::collections::HashMap::new(),
            prompt_file: None,
            inline_task: Some("Implement the feature".to_string()),
            role: "worker".to_string(),
            label: Some("Worker 1".to_string()),
        }
    }

    #[test]
    fn test_build_launch_command() {
        let adapter = OpenCodeAdapter;
        let spec = make_spec();
        let cmd = adapter.build_launch_command(&spec);

        assert_eq!(cmd.binary, "opencode");
        // Check YOLO env var is set
        assert_eq!(cmd.env.get("OPENCODE_YOLO"), Some(&"true".to_string()));
        // Check model flag
        assert!(cmd.args.contains(&"-m".to_string()));
        assert!(cmd.args.contains(&"claude-3.5-sonnet".to_string()));
        // Check prompt flag
        assert!(cmd.args.contains(&"--prompt".to_string()));
    }

    #[test]
    fn test_detect_completed() {
        let adapter = OpenCodeAdapter;

        assert_eq!(
            adapter.detect_status_signal("Task completed"),
            Some(AgentSignal::Completed)
        );

        assert_eq!(
            adapter.detect_status_signal("Done"),
            Some(AgentSignal::Completed)
        );
    }

    #[test]
    fn test_detect_tool_call() {
        let adapter = OpenCodeAdapter;

        match adapter.detect_status_signal("Executing: npm install") {
            Some(AgentSignal::ToolCall { tool }) => {
                assert_eq!(tool, "npm");
            }
            _ => panic!("Expected ToolCall signal"),
        }
    }

    #[test]
    fn test_yolo_env_var() {
        let adapter = OpenCodeAdapter;
        let spec = make_spec();
        let cmd = adapter.build_launch_command(&spec);

        // Verify YOLO mode is enabled via env var
        assert!(cmd.env.contains_key("OPENCODE_YOLO"));
    }

    #[test]
    fn test_cli_name() {
        let adapter = OpenCodeAdapter;
        assert_eq!(adapter.cli_name(), "opencode");
    }

    #[test]
    fn test_extract_opencode_tool_handles_unicode_before_marker() {
        assert_eq!(
            extract_opencode_tool("İ executing: npm install"),
            Some("npm".to_string())
        );
    }
}
