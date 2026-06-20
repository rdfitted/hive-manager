//! Unit-test PTY runtime stub that avoids linking portable-pty on Windows.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{AgentProcessStatus, LaunchSpec, LaunchedAgent, RuntimeAdapter, RuntimeError};

struct PtySession {
    _role: String,
    input: String,
}

impl PtySession {
    fn new(role: &str) -> Self {
        Self {
            _role: role.to_string(),
            input: String::new(),
        }
    }

    fn escape_batch_value(value: &str) -> String {
        let mut escaped = String::with_capacity(value.len());
        for ch in value.chars() {
            match ch {
                '%' => escaped.push_str("%%"),
                '"' | '^' | '&' | '|' | '<' | '>' | '(' | ')' => {
                    escaped.push('^');
                    escaped.push(ch);
                }
                _ => escaped.push(ch),
            }
        }
        escaped
    }

    fn quote_batch_argument(value: &str) -> String {
        format!("\"{}\"", Self::escape_batch_value(value))
    }

    fn apply_wsl_overrides(
        command: &str,
        args: &[String],
        wsl_distro: Option<&str>,
        wsl_binary_path: Option<&str>,
    ) -> Vec<String> {
        if !command.eq_ignore_ascii_case("wsl") {
            return args.to_vec();
        }

        let mut resolved = args.to_vec();

        let distro_override = wsl_distro
            .map(str::to_string)
            .or_else(|| std::env::var("HIVE_WSL_DISTRO").ok());
        if let Some(distro) = distro_override {
            if let Some(index) = resolved.iter().position(|arg| arg == "-d") {
                if index + 1 < resolved.len() {
                    resolved[index + 1] = distro;
                } else {
                    resolved.push(distro);
                }
            } else {
                resolved.insert(0, distro);
                resolved.insert(0, "-d".to_string());
            }
        }

        let binary_override = wsl_binary_path
            .map(str::to_string)
            .or_else(|| std::env::var("HIVE_WSL_BINARY_PATH").ok());
        if let Some(binary_path) = binary_override {
            if let Some(index) = resolved
                .iter()
                .position(|arg| arg == "/root/.local/bin/agent")
            {
                resolved[index] = binary_path;
            } else if resolved.len() >= 3 && resolved.first().map(|arg| arg.as_str()) == Some("-d")
            {
                resolved[2] = binary_path;
            }
        }

        resolved
    }

    fn create_batch_content(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> String {
        let mut lines = vec!["@echo off".to_string()];

        for (key, value) in env {
            lines.push(format!(
                "set \"{}={}\"",
                key,
                Self::escape_batch_value(value)
            ));
        }

        let mut command_line = Self::quote_batch_argument(command);
        for arg in args {
            command_line.push(' ');
            command_line.push_str(&Self::quote_batch_argument(arg));
        }
        lines.push(command_line);

        lines.join("\r\n")
    }
}

pub struct LocalPtyRuntime {
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
}

impl Default for LocalPtyRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalPtyRuntime {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn generate_process_id() -> String {
        format!("pty-{}", uuid::Uuid::new_v4())
    }
}

impl RuntimeAdapter for LocalPtyRuntime {
    fn launch(&self, spec: &LaunchSpec) -> Result<LaunchedAgent, RuntimeError> {
        let process_id = Self::generate_process_id();
        let session = PtySession::new(&spec.role);

        self.sessions
            .lock()
            .map_err(|_| RuntimeError::launch("Failed to lock sessions"))?
            .insert(process_id.clone(), session);

        Ok(LaunchedAgent {
            process_id,
            status: AgentProcessStatus::Starting,
        })
    }

    fn stop(&self, process_id: &str) -> Result<(), RuntimeError> {
        self.sessions
            .lock()
            .map_err(|_| RuntimeError::stop("Failed to lock sessions"))?
            .remove(process_id)
            .ok_or_else(|| RuntimeError::not_found(process_id))?;

        Ok(())
    }

    fn write(&self, process_id: &str, input: &str) -> Result<(), RuntimeError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| RuntimeError::write("Failed to lock sessions"))?;
        let session = sessions
            .get_mut(process_id)
            .ok_or_else(|| RuntimeError::not_found(process_id))?;

        session.input.push_str(input);

        Ok(())
    }

    fn resize(&self, process_id: &str, _cols: u16, _rows: u16) -> Result<(), RuntimeError> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| RuntimeError::resize("Failed to lock sessions"))?;

        if sessions.contains_key(process_id) {
            Ok(())
        } else {
            Err(RuntimeError::not_found(process_id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_pty_runtime_creation() {
        let runtime = LocalPtyRuntime::new();
        assert!(Arc::strong_count(&runtime.sessions) >= 1);
    }

    #[test]
    fn test_pty_session_batch_content() {
        let env: HashMap<String, String> = [("API_KEY".to_string(), "secret123".to_string())]
            .into_iter()
            .collect();

        let content = PtySession::create_batch_content(
            "claude",
            &["--model".to_string(), "opus".to_string()],
            &env,
        );

        assert!(content.contains("@echo off"));
        assert!(content.contains("set \"API_KEY=secret123\""));
        assert!(content.contains("\"claude\""));
        assert!(content.contains("\"--model\""));
        assert!(content.contains("\"opus\""));
    }

    #[test]
    fn test_pty_session_batch_content_escapes_windows_metacharacters() {
        let env: HashMap<String, String> = [("API_KEY".to_string(), "se%cr&et^\"".to_string())]
            .into_iter()
            .collect();

        let content = PtySession::create_batch_content(
            "C:\\Program Files\\Agent\\agent.exe",
            &["hello & goodbye".to_string(), "%TEMP%".to_string()],
            &env,
        );

        assert!(content.contains("set \"API_KEY=se%%cr^&et^^^\"\""));
        assert!(content.contains("\"C:\\Program Files\\Agent\\agent.exe\""));
        assert!(content.contains("\"hello ^& goodbye\""));
        assert!(content.contains("\"%%TEMP%%\""));
    }

    #[test]
    fn test_process_id_generation() {
        let id1 = LocalPtyRuntime::generate_process_id();
        let id2 = LocalPtyRuntime::generate_process_id();

        assert!(id1.starts_with("pty-"));
        assert!(id2.starts_with("pty-"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_apply_wsl_overrides() {
        let args = vec![
            "-d".to_string(),
            "Ubuntu".to_string(),
            "/root/.local/bin/agent".to_string(),
            "--force".to_string(),
        ];

        let resolved = PtySession::apply_wsl_overrides(
            "wsl",
            &args,
            Some("Ubuntu-24.04"),
            Some("/opt/cursor-agent"),
        );

        assert_eq!(resolved[1], "Ubuntu-24.04");
        assert_eq!(resolved[2], "/opt/cursor-agent");
    }
}
