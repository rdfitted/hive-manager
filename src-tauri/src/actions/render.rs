//! Helpers for wrapping real outputs in the `{ renderer?, data }` envelope.

use serde_json::{Map, Value};

pub fn envelope(data: Value, renderer: Option<&'static str>) -> Value {
    let mut object = Map::new();
    if let Some(renderer) = renderer {
        object.insert("renderer".to_string(), Value::String(renderer.to_string()));
    }
    object.insert("data".to_string(), data);
    Value::Object(object)
}

pub fn envelope_for_action_result(action_name: &str, data: Value) -> Value {
    envelope(data.clone(), renderer_for_action_result(action_name, &data))
}

pub fn envelope_for_content(data: Value, content: &str) -> Value {
    envelope(data, renderer_for_content(content))
}

fn renderer_for_action_result(action_name: &str, data: &Value) -> Option<&'static str> {
    if action_name.starts_with("git.") || action_name.contains("worktree") {
        return Some("diff");
    }

    if is_structured_table(data) {
        return Some("table");
    }

    None
}

fn is_structured_table(value: &Value) -> bool {
    match value {
        Value::Array(rows) if !rows.is_empty() => rows
            .iter()
            .all(|row| matches!(row, Value::Object(_) | Value::Array(_))),
        Value::Object(object) => object.values().any(is_structured_table),
        _ => false,
    }
}

fn renderer_for_content(content: &str) -> Option<&'static str> {
    if looks_like_diff(content) {
        return Some("diff");
    }

    if looks_like_markdown_table(content) {
        return Some("table");
    }

    None
}

fn looks_like_diff(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with("diff --git")
        || trimmed.starts_with("Index: ")
        || (content.contains("\n--- ") && content.contains("\n+++ "))
        || content.lines().any(|line| line.starts_with("@@ "))
}

fn looks_like_markdown_table(content: &str) -> bool {
    let mut previous_was_table_row = false;

    for line in content.lines() {
        let trimmed = line.trim();
        let is_table_row = trimmed.starts_with('|') && trimmed.ends_with('|');
        let is_separator = is_table_row
            && trimmed
                .trim_matches('|')
                .split('|')
                .all(|cell| cell.trim().chars().all(|ch| matches!(ch, '-' | ':' | ' ')));

        if previous_was_table_row && is_separator {
            return true;
        }

        previous_was_table_row = is_table_row;
    }

    false
}
