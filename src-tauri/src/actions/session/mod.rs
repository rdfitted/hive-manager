//! Session actions: thin wrappers over the existing [`SessionController`]
//! methods. The validators that were copy-pasted into both
//! `commands/session_commands.rs` and `http/handlers/sessions.rs` live here once,
//! on the input DTOs, and run via [`Action::validate_input`] before `run`.

use async_trait::async_trait;
use schemars::schema::RootSchema;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::http::handlers::{validate_cli, validate_project_path};
use crate::session::{DebateLaunchConfig, HiveLaunchConfig};

use super::error::ActionError;
use super::registry::{Action, ActionRegistry};
use super::ActionContext;

const SESSION_COLOR_ALLOWLIST: &[&str] = &[
    "#7aa2f7", "#bb9af7", "#9ece6a", "#e0af68", "#7dcfff", "#f7768e", "#ff9e64", "#f7b1d1",
];

/// Shared name validation (consolidated from the two former copies).
fn validate_session_name(name: Option<&str>) -> Result<(), ActionError> {
    let Some(name) = name else {
        return Ok(());
    };
    if name.trim().is_empty() {
        return Err(ActionError::bad_request(
            "Invalid session name: must not be empty or whitespace",
        ));
    }
    if name.chars().count() > 64 {
        return Err(ActionError::bad_request(
            "Invalid session name: must be 64 characters or fewer",
        ));
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err(ActionError::bad_request(
            "Invalid session name: must not contain '..', '/', or '\\'",
        ));
    }
    Ok(())
}

/// Shared color validation (consolidated from the two former copies).
fn validate_session_color(color: Option<&str>) -> Result<(), ActionError> {
    let Some(color) = color else {
        return Ok(());
    };
    if !SESSION_COLOR_ALLOWLIST.contains(&color) && !is_valid_hex_session_color(color) {
        return Err(ActionError::bad_request(format!(
            "Invalid session color '{}'. Valid options: {} or any #RRGGBB hex color",
            color,
            SESSION_COLOR_ALLOWLIST.join(", ")
        )));
    }
    Ok(())
}

fn is_valid_hex_session_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color.chars().skip(1).all(|c| c.is_ascii_hexdigit())
}

/// Validate a `HiveLaunchConfig` (consolidated from session_commands.rs).
fn validate_hive_launch_config(config: &HiveLaunchConfig) -> Result<(), ActionError> {
    validate_project_path(&config.project_path)?;
    validate_session_name(config.name.as_deref())?;
    validate_session_color(config.color.as_deref())?;
    validate_cli(&config.queen_config.cli)?;

    for worker in &config.workers {
        validate_cli(&worker.cli)?;
    }

    if let Some(evaluator_config) = &config.evaluator_config {
        if !evaluator_config.cli.trim().is_empty() {
            validate_cli(&evaluator_config.cli)?;
        }
    }

    if let Some(qa_workers) = &config.qa_workers {
        for qa_worker in qa_workers {
            validate_cli(&qa_worker.cli)?;
            match qa_worker.specialization.as_str() {
                "ui" | "api" | "a11y" => {}
                other => {
                    return Err(ActionError::bad_request(format!(
                        "Invalid QA specialization '{}'. Valid options: ui, api, a11y",
                        other
                    )));
                }
            }
        }
    }

    Ok(())
}

fn validate_debate_launch_config(config: &DebateLaunchConfig) -> Result<(), ActionError> {
    validate_project_path(&config.project_path)?;
    validate_session_name(config.name.as_deref())?;
    validate_session_color(config.color.as_deref())?;
    if config.topic.trim().is_empty() {
        return Err(ActionError::bad_request(
            "Debate launch requires a non-empty topic",
        ));
    }
    if config.debaters.is_empty() {
        return Err(ActionError::bad_request(
            "Debate launch requires at least one debater",
        ));
    }
    if config.rounds == 0 {
        return Err(ActionError::bad_request(
            "Debate launch requires at least one round",
        ));
    }
    validate_cli(&config.judge_config.cli)?;
    validate_cli(&config.default_cli)?;
    if let Some(queen_config) = &config.queen_config {
        validate_cli(&queen_config.cli)?;
    }
    for debater in &config.debaters {
        validate_cli(&debater.cli)?;
    }
    Ok(())
}

/// Input carrying just a session id (`session.get`, `session.stop`,
/// `session.close`).
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionIdInput {
    id: String,
}

fn validate_session_id_input(id: &str) -> Result<(), ActionError> {
    if id.contains("..") || id.contains('/') || id.contains('\\') {
        return Err(ActionError::bad_request(
            "Invalid session ID: must not contain '..', '/', or '\\'",
        ));
    }
    Ok(())
}

/// Empty input marker for actions that take no parameters (`session.list`).
#[derive(Debug, Deserialize, JsonSchema)]
struct EmptyInput {}

/// Input for `session.update_metadata`.
#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateMetadataInput {
    id: String,
    /// Outer `Some` means "set this field"; inner `None` clears it.
    #[serde(default)]
    name: Option<Option<String>>,
    #[serde(default)]
    color: Option<Option<String>>,
}

fn deserialize_input<T: for<'de> Deserialize<'de>>(input: Value) -> Result<T, ActionError> {
    serde_json::from_value(input)
        .map_err(|e| ActionError::bad_request(format!("Invalid input: {}", e)))
}

// ---------------------------------------------------------------------------
// session.list
// ---------------------------------------------------------------------------

struct ListSessions;

#[async_trait]
impl Action for ListSessions {
    fn name(&self) -> &'static str {
        "session.list"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(EmptyInput)
    }

    async fn run(&self, ctx: &ActionContext, _input: Value) -> Result<Value, ActionError> {
        let sessions = {
            let controller = ctx.state.session_controller.read();
            controller.list_sessions()
        };
        serde_json::to_value(sessions)
            .map_err(|e| ActionError::internal(format!("Failed to serialize sessions: {}", e)))
    }
}

// ---------------------------------------------------------------------------
// session.get
// ---------------------------------------------------------------------------

struct GetSession;

#[async_trait]
impl Action for GetSession {
    fn name(&self) -> &'static str {
        "session.get"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(SessionIdInput)
    }

    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        let parsed: SessionIdInput = deserialize_input(input.clone())?;
        validate_session_id_input(&parsed.id)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: SessionIdInput = deserialize_input(input)?;
        let session = {
            let controller = ctx.state.session_controller.read();
            controller.get_session(&parsed.id)
        };
        serde_json::to_value(session)
            .map_err(|e| ActionError::internal(format!("Failed to serialize session: {}", e)))
    }
}

// ---------------------------------------------------------------------------
// session.stop
// ---------------------------------------------------------------------------

struct StopSession;

#[async_trait]
impl Action for StopSession {
    fn name(&self) -> &'static str {
        "session.stop"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(SessionIdInput)
    }

    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        let parsed: SessionIdInput = deserialize_input(input.clone())?;
        validate_session_id_input(&parsed.id)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: SessionIdInput = deserialize_input(input)?;
        {
            let controller = ctx.state.session_controller.read();
            controller
                .stop_session(&parsed.id)
                .map_err(ActionError::from)?;
        }
        Ok(json!({ "message": format!("Session {} stopped", parsed.id) }))
    }
}

// ---------------------------------------------------------------------------
// session.close
// ---------------------------------------------------------------------------

struct CloseSession;

#[async_trait]
impl Action for CloseSession {
    fn name(&self) -> &'static str {
        "session.close"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(SessionIdInput)
    }

    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        let parsed: SessionIdInput = deserialize_input(input.clone())?;
        validate_session_id_input(&parsed.id)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: SessionIdInput = deserialize_input(input)?;
        {
            let controller = ctx.state.session_controller.read();
            controller.close_session(&parsed.id).map_err(|e| {
                if e.starts_with("Session not found") {
                    ActionError::not_found(e)
                } else {
                    ActionError::internal(e)
                }
            })?;
        }
        Ok(json!({ "message": format!("Session {} closed", parsed.id) }))
    }
}

// ---------------------------------------------------------------------------
// session.launch_hive_v2
// ---------------------------------------------------------------------------

struct LaunchHiveV2;

#[async_trait]
impl Action for LaunchHiveV2 {
    fn name(&self) -> &'static str {
        "session.launch_hive_v2"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(HiveLaunchConfig)
    }

    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        let config: HiveLaunchConfig = deserialize_input(input.clone())?;
        validate_hive_launch_config(&config)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let config: HiveLaunchConfig = deserialize_input(input)?;
        let session = {
            let controller = ctx.state.session_controller.read();
            controller
                .launch_hive_v2(config)
                .map_err(ActionError::from)?
        };
        serde_json::to_value(session)
            .map_err(|e| ActionError::internal(format!("Failed to serialize session: {}", e)))
    }
}

// ---------------------------------------------------------------------------
// session.update_metadata
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// session.launch_debate
// ---------------------------------------------------------------------------

struct LaunchDebate;

#[async_trait]
impl Action for LaunchDebate {
    fn name(&self) -> &'static str {
        "session.launch_debate"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(DebateLaunchConfig)
    }

    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        let config: DebateLaunchConfig = deserialize_input(input.clone())?;
        validate_debate_launch_config(&config)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let config: DebateLaunchConfig = deserialize_input(input)?;
        let session = {
            let controller = ctx.state.session_controller.read();
            controller
                .launch_debate(config)
                .map_err(ActionError::from)?
        };
        serde_json::to_value(session)
            .map_err(|e| ActionError::internal(format!("Failed to serialize session: {}", e)))
    }
}

struct UpdateSessionMetadata;

#[async_trait]
impl Action for UpdateSessionMetadata {
    fn name(&self) -> &'static str {
        "session.update_metadata"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(UpdateMetadataInput)
    }

    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        let parsed: UpdateMetadataInput = deserialize_input(input.clone())?;
        validate_session_id_input(&parsed.id)?;
        validate_session_name(parsed.name.as_ref().and_then(|value| value.as_deref()))?;
        validate_session_color(parsed.color.as_ref().and_then(|value| value.as_deref()))?;
        Ok(())
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: UpdateMetadataInput = deserialize_input(input)?;
        let session = {
            let controller = ctx.state.session_controller.read();
            controller
                .update_session_metadata(&parsed.id, parsed.name, parsed.color)
                .map_err(|e| {
                    if e.starts_with("Session not found") {
                        ActionError::not_found(e)
                    } else {
                        ActionError::internal(e)
                    }
                })?
        };
        serde_json::to_value(session)
            .map_err(|e| ActionError::internal(format!("Failed to serialize session: {}", e)))
    }
}

/// Register every session action into the registry.
pub fn register(registry: &mut ActionRegistry) {
    registry.register(Box::new(ListSessions));
    registry.register(Box::new(GetSession));
    registry.register(Box::new(StopSession));
    registry.register(Box::new(CloseSession));
    registry.register(Box::new(LaunchHiveV2));
    registry.register(Box::new(LaunchDebate));
    registry.register(Box::new(UpdateSessionMetadata));
}
