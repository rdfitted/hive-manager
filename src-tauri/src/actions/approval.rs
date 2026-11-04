//! Server-issued, expiring approval requests. Only the main frontend can decide.
use super::{Action, ActionContext, ActionError, ActionRegistry, Caller};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use schemars::{schema::RootSchema, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct PendingApproval {
    session_id: String,
    target_agent_id: String,
    instance_id: uuid::Uuid,
    title: String,
    description: String,
    pub(crate) expires_at: DateTime<Utc>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RequestApproval {
    session_id: String,
    target_agent_id: String,
    title: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approved,
    Rejected,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveApprovalRequest {
    pub session_id: String,
    pub approval_id: String,
    pub decision: ApprovalDecision,
}

fn validate_target(
    state: &crate::http::state::AppState,
    session_id: &str,
    agent_id: &str,
) -> Result<uuid::Uuid, ActionError> {
    let controller = state.session_controller.read();
    let session = controller
        .get_session(session_id)
        .ok_or_else(|| ActionError::conflict("Approval session is no longer active"))?;
    if matches!(
        session.state,
        crate::session::SessionState::Completed
            | crate::session::SessionState::Closing
            | crate::session::SessionState::Closed
            | crate::session::SessionState::Failed(_)
    ) {
        return Err(ActionError::conflict(
            "Approval session is no longer active",
        ));
    }
    if !session.agents.iter().any(|agent| agent.id == agent_id) {
        return Err(ActionError::conflict(
            "Approval target is not in this session",
        ));
    }
    state
        .pty_manager
        .read()
        .instance_id(agent_id)
        .ok_or_else(|| ActionError::conflict("Approval target is no longer running"))
}

struct CreateApproval;
#[async_trait]
impl Action for CreateApproval {
    fn name(&self) -> &'static str {
        "approval.request"
    }
    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(RequestApproval)
    }
    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let request: RequestApproval =
            serde_json::from_value(input).map_err(|e| ActionError::bad_request(e.to_string()))?;
        if request.title.trim().is_empty()
            || request.title.len() > 200
            || request.description.len() > 8192
            || request.session_id.contains(['/', '\\', '.'])
            || request.target_agent_id.contains(['/', '\\', '.'])
        {
            return Err(ActionError::bad_request("Invalid approval request"));
        }
        let matcher = ctx.state.pty_manager.read().heartbeat_credential_matcher();
        let values = [
            request.session_id.clone(),
            request.target_agent_id.clone(),
            request.title.clone(),
            request.description.clone(),
        ];
        let contains_credential =
            tokio::task::spawn_blocking(move || values.iter().any(|value| matcher.contains(value)))
                .await
                .map_err(|_| ActionError::internal("Approval credential scan unavailable"))?;
        if contains_credential {
            return Err(ActionError::bad_request(
                "Approval request must not contain a credential",
            ));
        }
        let instance_id =
            validate_target(&ctx.state, &request.session_id, &request.target_agent_id)?;
        let approval_id = uuid::Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::minutes(15);
        let pending = PendingApproval {
            session_id: request.session_id.clone(),
            target_agent_id: request.target_agent_id.clone(),
            instance_id,
            title: request.title.clone(),
            description: request.description.clone(),
            expires_at,
        };
        {
            let mut approvals = ctx.state.pending_approvals.lock();
            approvals.retain(|_, request| request.expires_at > Utc::now());
            if approvals.len() >= 1024 {
                return Err(ActionError::conflict("Too many pending approvals"));
            }
            approvals.insert(approval_id.clone(), pending);
        }
        let data = json!({"approvalId":approval_id, "title":request.title, "description":request.description,
            "targetAgentId":request.target_agent_id, "expiresAt":expires_at});
        let stored = ctx
            .state
            .storage
            .append_conversation_envelope(
                &request.session_id,
                &request.target_agent_id,
                "hive-manager",
                &request.title,
                Some("approval".into()),
                Some(data.clone()),
            )
            .await;
        let message = match stored {
            Ok(message) => message,
            Err(error) => {
                ctx.state.pending_approvals.lock().remove(&approval_id);
                return Err(ActionError::internal(error.to_string()));
            }
        };
        if let Err(error) = ctx
            .state
            .emit_conversation_message(&request.session_id, &request.target_agent_id, &message)
            .await
        {
            tracing::warn!("Approval event delivery failed: {error}");
        }
        Ok(json!({"renderer":"approval", "data":data}))
    }
}

struct ResolveApproval;
#[async_trait]
impl Action for ResolveApproval {
    fn name(&self) -> &'static str {
        "approval.resolve"
    }
    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(ResolveApprovalRequest)
    }
    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        if ctx.caller != Caller::Frontend {
            return Err(ActionError::bad_request(
                "Approval decisions are only available through Tauri commands",
            ));
        }
        let request: ResolveApprovalRequest =
            serde_json::from_value(input).map_err(|e| ActionError::bad_request(e.to_string()))?;
        let pending = {
            let mut approvals = ctx.state.pending_approvals.lock();
            let pending = approvals
                .get(&request.approval_id)
                .ok_or_else(|| ActionError::conflict("Approval is stale or already resolved"))?;
            if pending.session_id != request.session_id {
                return Err(ActionError::conflict("Approval belongs to another session"));
            }
            // Consume BEFORE any I/O. A failed send can already have delivered bytes.
            approvals
                .remove(&request.approval_id)
                .expect("pending approval held under lock")
        };
        if pending.expires_at <= Utc::now()
            || validate_target(&ctx.state, &pending.session_id, &pending.target_agent_id)?
                != pending.instance_id
        {
            return Err(ActionError::conflict(
                "Approval is stale; request a new approval",
            ));
        }
        let state = Arc::clone(&ctx.state);
        let result = tokio::task::spawn_blocking(move || -> Result<Value, ActionError> {
            let message = json!({"type":"approval_decision", "approval_id":request.approval_id,
                "decision":request.decision, "title":pending.title, "description":pending.description}).to_string();
            let manager = state.pty_manager.read();
            let protocol = manager.with_current_instance(&pending.target_agent_id, pending.instance_id,
                |_| manager.protocol_agent(&pending.target_agent_id))
                .ok_or_else(|| ActionError::conflict("Approval target runtime was replaced"))?;
            let (delivery, submitted) = if let Some(protocol) = protocol {
                if !protocol.transport.is_alive() { return Err(ActionError::conflict("Approval target stopped")); }
                let delivery = protocol.transport.send_input(&message, true).map_err(ActionError::internal)?;
                (Some(delivery), false)
            } else {
                let receipt = manager.submit_instance(&pending.target_agent_id, pending.instance_id, message.as_bytes())
                    .map_err(|e| ActionError::conflict(e.to_string()))?;
                (None, receipt.submit_bytes_written > 0)
            };
            drop(manager);
            let at = Utc::now();
            let controller = state.session_controller.read();
            controller.record_input_time_for_instance(&pending.session_id, &pending.target_agent_id, pending.instance_id, at);
            if let Some(delivery) = &delivery {
                controller.record_instance_injection_delivery(&pending.session_id, &pending.target_agent_id, pending.instance_id, at, delivery).map_err(ActionError::internal)?;
            } else {
                controller.record_instance_injection_submission(&pending.session_id, &pending.target_agent_id, pending.instance_id, at, submitted).map_err(ActionError::internal)?;
            }
            Ok(json!({"approval_id":request.approval_id, "decision":request.decision, "delivered":true}))
        }).await.map_err(|e| ActionError::internal(e.to_string()))?;
        result
    }
}

pub fn register(registry: &mut ActionRegistry) {
    registry.register(Box::new(CreateApproval));
    registry.register(Box::new(ResolveApproval));
}

#[tauri::command]
pub async fn resolve_approval(
    state: tauri::State<'_, Arc<crate::http::state::AppState>>,
    request: ResolveApprovalRequest,
) -> Result<Value, String> {
    let context = ActionContext::new(Caller::Frontend, Arc::clone(&state));
    state
        .registry()
        .dispatch(
            "approval.resolve",
            &context,
            serde_json::to_value(request).map_err(|e| e.to_string())?,
        )
        .await
        .map_err(|e| e.to_message())
}
