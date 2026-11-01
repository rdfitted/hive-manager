use crate::transport::{AgentEvent, AgentEventKind};
use serde_json::{json, Value};

pub(crate) fn result_error(agent: &str, value: &Value, turn: &str) -> Option<AgentEvent> {
    if value["is_error"] != true {
        return None;
    }
    let errors = value["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .filter_map(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let message = if !errors.is_empty() {
        errors.as_str()
    } else {
        value["result"]
            .as_str()
            .or_else(|| value["error"]["message"].as_str())
            .or_else(|| value["error"].as_str())
            .filter(|text| !text.trim().is_empty())
            .unwrap_or("Claude turn failed. Inspect the raw event log for details.")
    };
    Some(event(
        agent,
        AgentEventKind::Error,
        Some(format!("{turn}:error")),
        json!({"message":message,"text":message}),
    ))
}

pub(crate) fn event(
    agent: &str,
    kind: AgentEventKind,
    id: Option<String>,
    payload: Value,
) -> AgentEvent {
    AgentEvent {
        agent_id: agent.into(),
        cursor: String::new(),
        ts: chrono::Utc::now().to_rfc3339(),
        kind,
        source_id: id,
        truncated: false,
        payload,
    }
    .normalize()
}

pub(crate) fn server_reply(value: &Value) -> Value {
    let id = &value["request_id"];
    if value["request"]["subtype"] == "can_use_tool" {
        json!({"type":"control_response","response":{"subtype":"success","request_id":id,
            "response":{"behavior":"deny","message":"Interactive approval needs PTY; restart with PTY to approve."}}})
    } else {
        json!({"type":"control_response","response":{"subtype":"error","request_id":id,
            "error":"Unsupported host request; select PTY for interactive controls"}})
    }
}

/// Known root records only. Partial text and final snapshots share a message ID.
pub(crate) fn normalize(agent: &str, value: &Value, message_id: Option<&str>) -> Vec<AgentEvent> {
    use AgentEventKind::*;
    let mut events = Vec::new();
    let mut push = |kind, id, payload| events.push(event(agent, kind, id, payload));
    let kind = value["type"].as_str().unwrap_or("");
    if kind == "stream_event" {
        let wire = &value["event"];
        if wire["type"] == "content_block_delta" && wire["delta"]["type"] == "text_delta" {
            if let (Some(id), Some(text)) = (message_id, wire["delta"]["text"].as_str()) {
                push(AssistantDelta, Some(id.into()), json!({"text":text}));
            }
        }
    } else if kind == "assistant" || kind == "user" {
        let message = &value["message"];
        let id = message["id"]
            .as_str()
            .or_else(|| value["uuid"].as_str())
            .map(str::to_owned);
        let mut text = String::new();
        if let Some(content) = message["content"].as_str() {
            text.push_str(content);
        }
        if let Some(blocks) = message["content"].as_array() {
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(t) = block["text"].as_str() {
                            text.push_str(t);
                        }
                    }
                    Some("tool_use") if kind == "assistant" => {
                        push(
                            ToolStart,
                            block["id"].as_str().map(str::to_owned),
                            json!({"tool_id":block["id"],"name":block["name"],"input":block["input"]}),
                        );
                    }
                    Some("tool_result") if kind == "user" => {
                        push(
                            ToolResult,
                            block["tool_use_id"].as_str().map(str::to_owned),
                            json!({"tool_id":block["tool_use_id"],"result":block["content"],"is_error":block["is_error"].as_bool().unwrap_or(false)}),
                        );
                    }
                    _ => {}
                }
            }
        }
        if !text.is_empty() {
            push(
                if kind == "assistant" {
                    AssistantMessage
                } else {
                    UserMessage
                },
                id,
                json!({"text":text}),
            );
        }
    }
    events
}
