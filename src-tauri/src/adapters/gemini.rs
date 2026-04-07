//! Gemini CLI adapter implementation.

use super::{AgentLaunchSpec, AgentSignal, BootstrapContext, CliAdapter, LaunchCommand};

/// Gemini CLI adapter.
///
/// Gemini CLI-specific behavior:
/// - Auto-approve: `-y` (YOLO mode)
/// - Model flag: `-m`
/// - Prompts: `-i` flag for initial prompt
pub struct GeminiAdapter;

impl CliAdapter for GeminiAdapter {
    fn cli_name(&self) -> &'static str {
        "gemini"
    }

    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand {
        let mut cmd = LaunchCommand::new("gemini", spec.cwd.clone());

        // Add auto-approve flag (YOLO mode)
        cmd = cmd.arg("-y");

        // Add model if specified
        if let Some(ref model) = spec.model {
            cmd = cmd.arg("-m").arg(model);
        }

        // Add extra flags from spec
        cmd = cmd.args(spec.flags.iter().cloned());

        // Add environment variables
        cmd = cmd.envs(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));

        // Add prompt using -i flag for initial prompt
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

        // Gemini completion patterns
        if line_lower.contains("task completed") || line_lower.contains("finished") || line_lower.contains("done!") {
            return Some(AgentSignal::Completed);
        }

        // Error patterns
        if line_lower.contains("error:") || line_lower.contains("failed") || line_lower.contains("exception") {
            return Some(AgentSignal::Failed {
                message: line.to_string(),
            });
        }

        // Waiting for input patterns
        if line.contains("?") || line_lower.contains("please provide") {
            return Some(AgentSignal::WaitingInput);
        }

        // Tool call detection (Gemini format)
        if line_lower.contains("calling function:") || line_lower.contains("using tool:") {
            let tool = extract_gemini_tool(line).unwrap_or_else(|| "unknown".to_string());
            return Some(AgentSignal::ToolCall { tool });
        }

        // Processing indicator
        if line_lower.contains("generating") || line_lower.contains("thinking") {
            return Some(AgentSignal::Processing);
        }

        None
    }

    fn build_bootstrap_prompt(&self, context: &BootstrapContext) -> String {
        let mut prompt = format!(
            "You are a {} agent (Gemini) in session {}.\n",
            context.role, context.session_id
        );

        if let Some(ref task_file) = context.task_file {
            prompt.push_str(&format!(
                "Task file: {}\n",
                task_file.display()
            ));
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
        Some("-y")
    }

    fn model_flag(&self) -> Option<&'static str> {
        Some("-m")
    }

    fn prompt_flag(&self) -> Option<&'static str> {
        Some("-i")
    }
}

/// Extract tool/function name from Gemini output.
fn extract_gemini_tool(line: &str) -> Option<String> {
    let patterns = ["calling function:", "using tool:", "function:"];

    for pattern in patterns.iter() {
        if let Some(pos) = line.to_lowercase().find(pattern) {
            let rest = &line[pos + pattern.len()..];
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
            cli: "gemini".to_string(),
            model: Some("gemini-2.5-pro".to_string()),
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
        let adapter = GeminiAdapter;
        let spec = make_spec();
        let cmd = adapter.build_launch_command(&spec);

        assert_eq!(cmd.binary, "gemini");
        assert!(cmd.args.contains(&"-y".to_string()));
        assert!(cmd.args.contains(&"-m".to_string()));
        assert!(cmd.args.contains(&"gemini-2.5-pro".to_string()));
        assert!(cmd.args.contains(&"-i".to_string()));
    }

    #[test]
    fn test_detect_completed() {
        let adapter = GeminiAdapter;

        assert_eq!(
            adapter.detect_status_signal("Task completed"),
            Some(AgentSignal::Completed)
        );

        assert_eq!(
            adapter.detect_status_signal("Done!"),
            Some(AgentSignal::Completed)
        );
    }

    #[test]
    fn test_detect_tool_call() {
        let adapter = GeminiAdapter;

        match adapter.detect_status_signal("Calling function: read_file") {
            Some(AgentSignal::ToolCall { tool }) => {
                assert_eq!(tool, "read_file");
            }
            _ => panic!("Expected ToolCall signal"),
        }
    }

    #[test]
    fn test_cli_name() {
        let adapter = GeminiAdapter;
        assert_eq!(adapter.cli_name(), "gemini");
    }
}
