//! Droid CLI adapter implementation.

use super::{AgentLaunchSpec, AgentSignal, BootstrapContext, CliAdapter, LaunchCommand};

/// Droid CLI adapter.
///
/// Droid CLI-specific behavior:
/// - Auto-approve: None (interactive mode only)
/// - Model: Selected via /model command or config
/// - Prompts: positional argument
///
/// Note: Droid is interactive by design - no auto-approve flag available.
pub struct DroidAdapter;

impl CliAdapter for DroidAdapter {
    fn cli_name(&self) -> &'static str {
        "droid"
    }

    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand {
        let mut cmd = LaunchCommand::new("droid", spec.cwd.clone());

        // Droid has no auto-approve flag - it's interactive by design
        // Model is selected via /model command or config, not CLI flag

        // Add extra flags from spec
        cmd = cmd.args(spec.flags.iter().cloned());

        // Add environment variables
        cmd = cmd.envs(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));

        // Add prompt (positional for Droid)
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

        // Droid completion patterns
        if line_lower.contains("task completed") || line_lower.contains("all done") || line_lower.contains("finished successfully") {
            return Some(AgentSignal::Completed);
        }

        // Error patterns
        if line_lower.contains("error:") || line_lower.contains("failed:") || line_lower.contains("an error occurred") {
            return Some(AgentSignal::Failed {
                message: line.to_string(),
            });
        }

        // Waiting for input patterns (Droid uses various prompts)
        if line.contains("?") && (line.contains("[") || line.contains("(y/n)")) {
            return Some(AgentSignal::WaitingInput);
        }

        // Tool call detection (Droid format)
        if line_lower.contains("using tool:") || line_lower.contains("calling:") {
            let tool = extract_droid_tool(line).unwrap_or_else(|| "unknown".to_string());
            return Some(AgentSignal::ToolCall { tool });
        }

        // Processing indicator
        if line_lower.contains("thinking") || line_lower.contains("processing") || line_lower.contains("generating") {
            return Some(AgentSignal::Processing);
        }

        None
    }

    fn build_bootstrap_prompt(&self, context: &BootstrapContext) -> String {
        let mut prompt = format!(
            "You are a {} agent (Droid) in session {}.\n",
            context.role, context.session_id
        );

        if let Some(ref task_file) = context.task_file {
            prompt.push_str(&format!(
                "Task instructions: {}\n",
                task_file.display()
            ));
        }

        prompt.push_str(&format!(
            "Project root: {}\n",
            context.project_path.display()
        ));

        for (key, value) in &context.variables {
            prompt.push_str(&format!("{}: {}\n", key, value));
        }

        prompt
    }

    // Droid has no auto-approve flag
    fn auto_approve_flag(&self) -> Option<&'static str> {
        None
    }

    // Droid has no model flag - uses /model command
    fn model_flag(&self) -> Option<&'static str> {
        None
    }
}

/// Extract tool name from Droid output.
fn extract_droid_tool(line: &str) -> Option<String> {
    let patterns = ["using tool:", "calling:"];
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
            cli: "droid".to_string(),
            model: None, // Droid doesn't use model flag
            flags: vec![],
            cwd: PathBuf::from("/project"),
            env: std::collections::HashMap::new(),
            prompt_file: None,
            inline_task: Some("Review the code".to_string()),
            role: "reviewer".to_string(),
            label: Some("Reviewer".to_string()),
        }
    }

    #[test]
    fn test_build_launch_command() {
        let adapter = DroidAdapter;
        let spec = make_spec();
        let cmd = adapter.build_launch_command(&spec);

        assert_eq!(cmd.binary, "droid");
        // No auto-approve flag
        assert!(!cmd.args.iter().any(|a| a.contains("dangerously") || a == "-y"));
        // Has the task
        assert!(cmd.args.contains(&"Review the code".to_string()));
    }

    #[test]
    fn test_detect_completed() {
        let adapter = DroidAdapter;

        assert_eq!(
            adapter.detect_status_signal("Task completed successfully"),
            Some(AgentSignal::Completed)
        );

        assert_eq!(
            adapter.detect_status_signal("All done!"),
            Some(AgentSignal::Completed)
        );
    }

    #[test]
    fn test_detect_failed() {
        let adapter = DroidAdapter;

        match adapter.detect_status_signal("Error: Connection failed") {
            Some(AgentSignal::Failed { message }) => {
                assert!(message.contains("Error"));
            }
            _ => panic!("Expected Failed signal"),
        }
    }

    #[test]
    fn test_no_auto_approve() {
        let adapter = DroidAdapter;
        assert_eq!(adapter.auto_approve_flag(), None);
    }

    #[test]
    fn test_cli_name() {
        let adapter = DroidAdapter;
        assert_eq!(adapter.cli_name(), "droid");
    }

    #[test]
    fn test_extract_droid_tool_handles_unicode_before_marker() {
        assert_eq!(
            extract_droid_tool("İ calling: list_files"),
            Some("list_files".to_string())
        );
    }
}
