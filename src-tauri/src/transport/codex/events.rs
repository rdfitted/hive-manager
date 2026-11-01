use crate::transport::{AgentEvent, AgentEventKind};
use serde_json::{json, Value};

pub(crate) fn completion_error(
    agent: &str,
    held_thread: &str,
    method: &str,
    params: &Value,
    child: bool,
) -> Option<AgentEvent> {
    if child || method != "turn/completed" || params["threadId"] != held_thread {
        return None;
    }
    let turn = &params["turn"];
    if turn["status"] != "failed" && turn["error"].is_null() {
        return None;
    }
    let message = turn["error"]["message"]
        .as_str()
        .or_else(|| turn["error"].as_str())
        .filter(|text| !text.trim().is_empty())
        .unwrap_or("Codex turn failed. Inspect the raw event log for details.");
    Some(event(
        agent,
        AgentEventKind::Error,
        turn["id"]
            .as_str()
            .map(|id| format!("{held_thread}:{id}:error")),
        json!({"message":message,"text":message}),
    ))
}

pub(crate) fn event(
    agent: &str,
    kind: AgentEventKind,
    source: Option<String>,
    payload: Value,
) -> AgentEvent {
    AgentEvent {
        agent_id: agent.into(),
        cursor: String::new(),
        ts: chrono::Utc::now().to_rfc3339(),
        kind,
        source_id: source,
        truncated: false,
        payload,
    }
    .normalize()
}

pub(crate) fn normalize(
    agent: &str,
    held_thread: &str,
    method: &str,
    params: &Value,
    child: bool,
) -> Option<AgentEvent> {
    let thread = params.get("threadId")?.as_str()?;
    if child {
        return matches!(method, "turn/started" | "turn/completed" | "item/started" | "item/completed").then(|| event(agent, AgentEventKind::Status,
            Some(format!("{held_thread}:child:{thread}")), json!({"status":"subagent_activity","message":"Sub-agent activity","parent_thread_id":held_thread})));
    }
    if thread != held_thread {
        return None;
    }
    let item = &params["item"];
    let source = item["id"]
        .as_str()
        .or_else(|| params["itemId"].as_str())
        .or_else(|| params["turn"]["id"].as_str())
        .map(|id| format!("{held_thread}:{id}"));
    let (kind, payload) = match method {
        "turn/started" => (AgentEventKind::TurnStart, json!({"status":"started"})),
        "turn/completed" => (
            AgentEventKind::TurnEnd,
            json!({"status":params["turn"]["status"],"error":params["turn"]["error"]}),
        ),
        "item/agentMessage/delta" => (
            AgentEventKind::AssistantDelta,
            json!({"text":params["delta"].as_str().unwrap_or("")}),
        ),
        "item/commandExecution/outputDelta" => (
            AgentEventKind::ToolResult,
            json!({"tool_id":params["itemId"],"result":params["delta"],"is_error":false}),
        ),
        "item/started" | "item/completed" => match item["type"].as_str()? {
            "userMessage" if method == "item/completed" => {
                let text = item["content"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                (AgentEventKind::UserMessage, json!({"text":text}))
            }
            "agentMessage" if method == "item/completed" => (
                AgentEventKind::AssistantMessage,
                json!({"text":item["text"].as_str().unwrap_or("")}),
            ),
            "commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall" => {
                if method == "item/started" {
                    (
                        AgentEventKind::ToolStart,
                        json!({"tool_id":item["id"],"name":item["type"],"input":item.get("command").or_else(||item.get("arguments")).cloned().unwrap_or(Value::Null)}),
                    )
                } else {
                    (
                        AgentEventKind::ToolResult,
                        json!({"tool_id":item["id"],"result":item.get("aggregatedOutput").or_else(||item.get("result")).or_else(||item.get("changes")).cloned().unwrap_or_else(||item["status"].clone()),"is_error":item["status"]=="failed"}),
                    )
                }
            }
            "subAgentActivity" | "collabAgentToolCall" => (
                AgentEventKind::Status,
                json!({"status":"subagent_activity","message":"Sub-agent activity","parent_thread_id":held_thread}),
            ),
            _ => return None,
        },
        "error" => (
            AgentEventKind::Error,
            json!({"text":"Codex reported an error; inspect the raw event log or select PTY."}),
        ),
        _ => return None,
    };
    Some(event(agent, kind, source, payload))
}

/// Every server request is answered, including requests for unheld threads.
pub(crate) fn server_reply(request: &Value) -> Value {
    let id = request["id"].clone();
    let result = match request["method"].as_str().unwrap_or("") {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            json!({"decision":"decline"})
        }
        "execCommandApproval" | "applyPatchApproval" => json!({"decision":"abort"}),
        "item/permissions/requestApproval" => json!({"permissions":{},"scope":"turn"}),
        "item/tool/requestUserInput" => json!({"answers":{}}),
        "mcpServer/elicitation/request" => json!({"action":"decline","content":null,"_meta":null}),
        "currentTime/read" => {
            return json!({"id":id,"error":{"code":-32601,"message":"Host clock service not implemented"}})
        }
        _ => {
            return json!({"id":id,"error":{"code":-32601,"message":"Request not supported by Hive; use PTY for interactive input"}})
        }
    };
    json!({"id":id,"result":result})
}
