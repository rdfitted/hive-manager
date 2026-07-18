//! PTY actions behind the unified action registry.

use async_trait::async_trait;
use schemars::schema::RootSchema;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::pty::AgentRole;

use super::error::ActionError;
use super::registry::{Action, ActionRegistry};
use super::{ActionContext, Caller};

const MAX_PASTE_SIZE: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum CreatePtyRole {
    ScratchShell,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ScratchShell {
    Powershell,
    Cmd,
}

impl ScratchShell {
    fn command(self) -> &'static str {
        match self {
            Self::Powershell => "powershell.exe",
            Self::Cmd => "cmd.exe",
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            Self::Powershell => &["-NoLogo"],
            Self::Cmd => &[],
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreatePtyInput {
    id: String,
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
    role: Option<CreatePtyRole>,
    shell: Option<ScratchShell>,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PtyDataInput {
    id: String,
    data: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct InjectInput {
    id: String,
    message: String,
    send_enter: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ResizeInput {
    id: String,
    cols: u16,
    rows: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PtyIdInput {
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EmptyInput {}

fn deserialize_input<T: for<'de> Deserialize<'de>>(input: Value) -> Result<T, ActionError> {
    serde_json::from_value(input)
        .map_err(|e| ActionError::bad_request(format!("Invalid input: {}", e)))
}

fn check_data_size(data_len: usize) -> Result<(), ActionError> {
    if data_len > MAX_PASTE_SIZE {
        return Err(ActionError::bad_request(format!(
            "Data size {} bytes exceeds maximum allowed {} bytes",
            data_len, MAX_PASTE_SIZE
        )));
    }

    Ok(())
}

fn require_frontend(ctx: &ActionContext) -> Result<(), ActionError> {
    if matches!(ctx.caller, Caller::Frontend) {
        Ok(())
    } else {
        Err(ActionError::bad_request(
            "PTY actions are only available through Tauri commands",
        ))
    }
}

fn resolve_create_role(
    input: &CreatePtyInput,
) -> Result<(AgentRole, Option<String>), ActionError> {
    match input.role {
        None => {
            if input.shell.is_some() || input.session_id.is_some() {
                return Err(ActionError::bad_request(
                    "shell and session_id require role=scratch_shell",
                ));
            }

            Ok((
                AgentRole::Worker {
                    index: 0,
                    parent: None,
                },
                None,
            ))
        }
        Some(CreatePtyRole::ScratchShell) => {
            let session_id = input
                .session_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    ActionError::bad_request("session_id is required for a scratch PTY")
                })?;
            let shell = input.shell.ok_or_else(|| {
                ActionError::bad_request("shell is required for a scratch PTY")
            })?;
            let id_prefix = format!("scratch:{session_id}:");
            let unique_id = input.id.strip_prefix(&id_prefix).unwrap_or_default();

            if session_id.contains(':') || unique_id.is_empty() || unique_id.contains(':') {
                return Err(ActionError::bad_request(format!(
                    "scratch PTY id must use the namespace {id_prefix}<unique-id-without-colons>"
                )));
            }
            if !input.command.eq_ignore_ascii_case(shell.command())
                || !input
                    .args
                    .iter()
                    .map(String::as_str)
                    .eq(shell.args().iter().copied())
            {
                return Err(ActionError::bad_request(format!(
                    "scratch shell metadata does not match {} {:?}",
                    shell.command(),
                    shell.args()
                )));
            }

            Ok((AgentRole::ScratchShell, Some(session_id.to_string())))
        }
    }
}

#[cfg(test)]
pub(super) fn resolve_create_role_for_test(
    input: Value,
) -> Result<(AgentRole, Option<String>), ActionError> {
    let parsed: CreatePtyInput = deserialize_input(input)?;
    resolve_create_role(&parsed)
}

struct CreatePty;

#[async_trait]
impl Action for CreatePty {
    fn name(&self) -> &'static str {
        "pty.create"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(CreatePtyInput)
    }

    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        let parsed: CreatePtyInput = deserialize_input(input.clone())?;
        resolve_create_role(&parsed)?;
        Ok(())
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: CreatePtyInput = deserialize_input(input)?;
        let (role, scratch_session_id) = resolve_create_role(&parsed)?;
        let session_controller = ctx.state.session_controller.read();
        let lifecycle_lock = scratch_session_id
            .as_deref()
            .map(|session_id| session_controller.session_lifecycle_lock(session_id));
        let _lifecycle_guard = lifecycle_lock.as_ref().map(|lock| lock.lock());
        let scratch_creation_guard = scratch_session_id
            .as_deref()
            .map(|session_id| {
                session_controller.reserve_scratch_pty(session_id, parsed.id.clone())
            })
            .transpose()
            .map_err(ActionError::bad_request)?;

        let args_refs: Vec<&str> = parsed.args.iter().map(String::as_str).collect();
        let create_result = {
            let pty_manager = ctx.state.pty_manager.read();
            pty_manager.create_session(
                parsed.id.clone(),
                role,
                &parsed.command,
                &args_refs,
                parsed.cwd.as_deref(),
                parsed.cols,
                parsed.rows,
            )
        };

        if let Err(error) = create_result {
            if scratch_creation_guard.is_some() {
                session_controller.unregister_scratch_pty(&parsed.id);
            }
            return Err(ActionError::internal(error.to_string()));
        }

        // Drop the barrier only after both the process and its ownership record exist.
        drop(scratch_creation_guard);
        Ok(Value::String(parsed.id))
    }
}

struct WritePty;

#[async_trait]
impl Action for WritePty {
    fn name(&self) -> &'static str {
        "pty.write"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(PtyDataInput)
    }

    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        let parsed: PtyDataInput = deserialize_input(input.clone())?;
        check_data_size(parsed.data.len())
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: PtyDataInput = deserialize_input(input)?;
        let pty_manager = ctx.state.pty_manager.read();
        pty_manager
            .write(&parsed.id, parsed.data.as_bytes())
            .map_err(|e| ActionError::internal(e.to_string()))?;
        Ok(Value::Null)
    }
}

struct PastePty;

#[async_trait]
impl Action for PastePty {
    fn name(&self) -> &'static str {
        "pty.paste"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(PtyDataInput)
    }

    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        let parsed: PtyDataInput = deserialize_input(input.clone())?;
        check_data_size(parsed.data.len())
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: PtyDataInput = deserialize_input(input)?;
        let pty_manager = ctx.state.pty_manager.read();
        pty_manager
            .write_bracketed(&parsed.id, parsed.data.as_bytes())
            .map_err(|e| ActionError::internal(e.to_string()))?;
        Ok(Value::Null)
    }
}

struct InjectPty;

#[async_trait]
impl Action for InjectPty {
    fn name(&self) -> &'static str {
        "pty.inject"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(InjectInput)
    }

    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        let parsed: InjectInput = deserialize_input(input.clone())?;
        check_data_size(parsed.message.len())
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: InjectInput = deserialize_input(input)?;
        let pty_manager = ctx.state.pty_manager.read();

        tracing::info!(
            "inject_to_pty: id={}, message_len={}, send_enter={}",
            parsed.id,
            parsed.message.len(),
            parsed.send_enter
        );

        if parsed.send_enter {
            let message_with_enter = format!("{}\r", parsed.message);
            pty_manager
                .write_bracketed(&parsed.id, message_with_enter.as_bytes())
                .map_err(|e| ActionError::internal(e.to_string()))?;
        } else {
            pty_manager
                .write_bracketed(&parsed.id, parsed.message.as_bytes())
                .map_err(|e| ActionError::internal(e.to_string()))?;
        }

        Ok(Value::Null)
    }
}

struct ResizePty;

#[async_trait]
impl Action for ResizePty {
    fn name(&self) -> &'static str {
        "pty.resize"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(ResizeInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: ResizeInput = deserialize_input(input)?;
        let pty_manager = ctx.state.pty_manager.read();
        pty_manager
            .resize(&parsed.id, parsed.cols, parsed.rows)
            .map_err(|e| ActionError::internal(e.to_string()))?;
        Ok(Value::Null)
    }
}

struct KillPty;

#[async_trait]
impl Action for KillPty {
    fn name(&self) -> &'static str {
        "pty.kill"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(PtyIdInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: PtyIdInput = deserialize_input(input)?;
        let session_controller = ctx.state.session_controller.read();
        let lifecycle_lock = session_controller.scratch_pty_lifecycle_lock(&parsed.id);
        let _lifecycle_guard = lifecycle_lock.as_ref().map(|lock| lock.lock());
        ctx.state
            .pty_manager
            .read()
            .kill(&parsed.id)
            .map_err(|e| ActionError::internal(e.to_string()))?;
        session_controller.unregister_scratch_pty(&parsed.id);
        Ok(Value::Null)
    }
}

struct PtyStatus;

#[async_trait]
impl Action for PtyStatus {
    fn name(&self) -> &'static str {
        "pty.status"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(PtyIdInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: PtyIdInput = deserialize_input(input)?;
        let pty_manager = ctx.state.pty_manager.read();
        serde_json::to_value(pty_manager.get_status(&parsed.id))
            .map_err(|e| ActionError::internal(format!("Failed to serialize PTY status: {}", e)))
    }
}

struct ListPtys;

#[async_trait]
impl Action for ListPtys {
    fn name(&self) -> &'static str {
        "pty.list"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(EmptyInput)
    }

    async fn run(&self, ctx: &ActionContext, _input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let pty_manager = ctx.state.pty_manager.read();
        serde_json::to_value(pty_manager.list_sessions())
            .map_err(|e| ActionError::internal(format!("Failed to serialize PTYs: {}", e)))
    }
}

pub fn register(registry: &mut ActionRegistry) {
    registry.register(Box::new(CreatePty));
    registry.register(Box::new(WritePty));
    registry.register(Box::new(PastePty));
    registry.register(Box::new(InjectPty));
    registry.register(Box::new(ResizePty));
    registry.register(Box::new(KillPty));
    registry.register(Box::new(PtyStatus));
    registry.register(Box::new(ListPtys));
}
