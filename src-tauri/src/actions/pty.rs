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

#[derive(Debug, Deserialize, JsonSchema)]
struct CreatePtyInput {
    id: String,
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
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

struct CreatePty;

#[async_trait]
impl Action for CreatePty {
    fn name(&self) -> &'static str {
        "pty.create"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(CreatePtyInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: CreatePtyInput = deserialize_input(input)?;
        let args_refs: Vec<&str> = parsed.args.iter().map(String::as_str).collect();
        let pty_manager = ctx.state.pty_manager.read();
        let id = pty_manager
            .create_session(
                parsed.id,
                AgentRole::Worker {
                    index: 0,
                    parent: None,
                },
                &parsed.command,
                &args_refs,
                parsed.cwd.as_deref(),
                parsed.cols,
                parsed.rows,
            )
            .map_err(|e| ActionError::internal(e.to_string()))?;
        Ok(Value::String(id))
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
        let pty_manager = ctx.state.pty_manager.read();
        pty_manager
            .kill(&parsed.id)
            .map_err(|e| ActionError::internal(e.to_string()))?;
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
