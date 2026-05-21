//! Antigravity CLI (`agy`) adapter — successor to the deprecated Gemini CLI.
//!
//! Antigravity CLI-specific behavior:
//! - Binary: `agy`
//! - Auto-approve: `--dangerously-skip-permissions` (NOT `-y` like the old gemini CLI)
//! - Model selection: **no flag**. Set in `~/.gemini/antigravity-cli/settings.json`
//!   under the `"model"` key. Per-invocation override is not supported by agy.
//! - Verbosity: **no flag**. Same settings.json, `"verbosity"` key.
//! - Initial interactive prompt: `-i` (alias for `--prompt-interactive`)
//! - Non-interactive single-shot: `-p` (alias for `--print`)
//!
//! Verified against `agy v1.0.0`. Google's Gemini CLI deprecates on 2026-06-18.

use super::{AgentLaunchSpec, AgentSignal, BootstrapContext, CliAdapter, LaunchCommand};

/// Antigravity CLI (`agy`) adapter.
pub struct AntigravityAdapter;

impl CliAdapter for AntigravityAdapter {
    fn cli_name(&self) -> &'static str {
        "antigravity"
    }

    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand {
        let mut cmd = LaunchCommand::new("agy", spec.cwd.clone());

        cmd = cmd.arg("--dangerously-skip-permissions");

        // NOTE: `agy` has no `--model` flag. Model is configured globally in
        // `~/.gemini/antigravity-cli/settings.json`. `spec.model` is intentionally
        // ignored for this CLI. The frontend hides the model field for antigravity
        // workers to reflect this.

        cmd = cmd.args(spec.flags.iter().cloned());
        cmd = cmd.envs(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));

        // inline_task is for solo/single-shot mode — use `-p` (--print) for
        // non-interactive execution, matching SessionController::add_inline_task_to_args.
        // prompt_file is for worker mode where the agent stays alive watching the
        // file — use `-i` (--prompt-interactive) so the session continues.
        //
        // KNOWN UPSTREAM ISSUE: google-antigravity/antigravity-cli#76 — `agy -p`
        // silently drops stdout when run with a non-TTY (pipe, subprocess, redirect).
        // Hive worker mode is unaffected because PTY-spawned sessions are TTY-like,
        // but Solo-mode antigravity launches (which use this `-p` branch) may
        // produce empty output until upstream lands a fix. Until then, prefer
        // `gemini` for Solo mode if you need captured stdout.
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
        None
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
            model: Some("Gemini 3.1 Pro".to_string()),
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
    fn test_build_launch_command_uses_agy_binary() {
        let adapter = AntigravityAdapter;
        // make_spec() has prompt_file set — interactive worker path.
        let spec = make_spec();
        let cmd = adapter.build_launch_command(&spec);

        assert_eq!(cmd.binary, "agy");
        assert!(cmd.args.contains(&"--dangerously-skip-permissions".to_string()));
        // prompt_file path uses -i (interactive); inline_task path uses -p (single-shot).
        assert!(cmd.args.contains(&"-i".to_string()));
    }

    #[test]
    fn test_build_launch_command_drops_model_flag() {
        let adapter = AntigravityAdapter;
        let spec = make_spec();
        let cmd = adapter.build_launch_command(&spec);

        assert!(
            !cmd.args.contains(&"-m".to_string()),
            "agy must not receive -m flag; model is set via settings.json"
        );
        assert!(
            !cmd.args.contains(&"--model".to_string()),
            "agy must not receive --model flag; model is set via settings.json"
        );
        assert!(
            !cmd.args.contains(&"Gemini 3.1 Pro".to_string()),
            "Model identifier from spec must not leak into args"
        );
    }

    #[test]
    fn test_inline_task_uses_p_flag_for_single_shot() {
        // inline_task is single-shot/solo mode — agy's -p (--print) runs the
        // prompt non-interactively. Matches SessionController::add_inline_task_to_args.
        let adapter = AntigravityAdapter;
        let mut spec = make_spec();
        spec.prompt_file = None;
        spec.inline_task = Some("Do the thing".to_string());
        let cmd = adapter.build_launch_command(&spec);

        let p_pos = cmd.args.iter().position(|a| a == "-p").expect("-p present");
        assert_eq!(cmd.args.get(p_pos + 1), Some(&"Do the thing".to_string()));
        assert!(
            !cmd.args.contains(&"-i".to_string()),
            "inline_task must not use -i; that's reserved for interactive worker mode (prompt_file path)"
        );
    }

    #[test]
    fn test_prompt_file_uses_i_flag_for_interactive_worker() {
        // prompt_file path keeps the worker alive watching the file — interactive.
        let adapter = AntigravityAdapter;
        let spec = make_spec(); // has prompt_file set, no inline_task
        let cmd = adapter.build_launch_command(&spec);

        assert!(cmd.args.contains(&"-i".to_string()));
        assert!(
            !cmd.args.contains(&"-p".to_string()),
            "prompt_file path must use -i; -p is for single-shot inline_task only"
        );
    }

    #[test]
    fn test_detect_completed() {
        let adapter = AntigravityAdapter;

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
        let adapter = AntigravityAdapter;

        match adapter.detect_status_signal("Calling function: read_file") {
            Some(AgentSignal::ToolCall { tool }) => {
                assert_eq!(tool, "read_file");
            }
            other => panic!("Expected ToolCall signal, got {:?}", other),
        }
    }

    #[test]
    fn test_cli_name() {
        let adapter = AntigravityAdapter;
        assert_eq!(adapter.cli_name(), "antigravity");
    }

    #[test]
    fn test_model_flag_is_none() {
        let adapter = AntigravityAdapter;
        assert!(
            adapter.model_flag().is_none(),
            "agy has no model flag; model lives in settings.json"
        );
    }

    #[test]
    fn test_auto_approve_flag() {
        let adapter = AntigravityAdapter;
        assert_eq!(adapter.auto_approve_flag(), Some("--dangerously-skip-permissions"));
    }

    #[test]
    fn test_detect_waiting_input_requires_explicit_prompt_marker() {
        let adapter = AntigravityAdapter;

        assert_eq!(adapter.detect_status_signal("What changed in this file?"), None);
        assert_eq!(
            adapter.detect_status_signal("Input:"),
            Some(AgentSignal::WaitingInput)
        );
    }

    #[test]
    fn test_extract_tool_handles_unicode_before_marker() {
        assert_eq!(
            extract_tool_name("İ calling function: read_file"),
            Some("read_file".to_string())
        );
    }
}
