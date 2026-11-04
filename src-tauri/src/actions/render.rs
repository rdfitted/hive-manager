//! Normalize real result payloads for conversation widgets.
use serde_json::{json, Value};

pub fn envelope(data: Value, renderer: Option<&str>) -> Value {
    match renderer {
        Some(renderer) => json!({"renderer": renderer, "data": data}),
        None => json!({"data": data}),
    }
}

pub fn envelope_for_action_result(_action_name: &str, data: Value) -> Value {
    // Actions that intentionally publish an envelope (approval.request) retain it.
    if data.get("renderer").is_some_and(Value::is_string) && data.get("data").is_some() {
        return data;
    }
    if let Some(text) = data.as_str() {
        let (renderer, payload) = content_payload(text);
        if renderer.is_some() {
            return envelope(payload.unwrap_or(data), renderer.as_deref());
        }
    }
    let renderer = if data.get("unified").is_some_and(Value::is_string) {
        Some("diff")
    } else if data
        .as_array()
        .is_some_and(|rows| !rows.is_empty() && rows.iter().all(Value::is_object))
        || data.get("rows").is_some_and(Value::is_array)
    {
        Some("table")
    } else {
        None
    };
    envelope(data, renderer)
}

/// Plain prose remains text; widgets receive only their supported data shapes.
pub fn content_payload(content: &str) -> (Option<String>, Option<Value>) {
    let trimmed = content.trim_start();
    if trimmed.starts_with("diff --git")
        || trimmed.starts_with("Index: ")
        || (trimmed.starts_with("--- ") && content.contains("\n+++ "))
        || content.lines().any(|line| line.starts_with("@@ "))
    {
        return (Some("diff".into()), Some(json!({"unified": content})));
    }
    let lines: Vec<_> = content.lines().map(str::trim).collect();
    for (index, pair) in lines.windows(2).enumerate() {
        let cells = |line: &str| -> Vec<String> {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect()
        };
        if !pair
            .iter()
            .all(|line| line.starts_with('|') && line.ends_with('|'))
        {
            continue;
        }
        let separators = cells(pair[1]);
        if !separators
            .iter()
            .all(|cell| cell.contains('-') && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
        {
            continue;
        }
        let columns = cells(pair[0]);
        if columns.len() != separators.len() {
            continue;
        }
        let unique: std::collections::HashSet<_> = columns.iter().collect();
        if columns.iter().any(String::is_empty) || unique.len() != columns.len() {
            continue;
        }
        let rows: Vec<Value> = lines[index + 2..]
            .iter()
            .take_while(|line| line.starts_with('|') && line.ends_with('|'))
            .map(|line| {
                let values = cells(line);
                Value::Object(
                    columns
                        .iter()
                        .enumerate()
                        .map(|(i, key)| {
                            (
                                key.clone(),
                                Value::String(values.get(i).cloned().unwrap_or_default()),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        return (
            Some("table".into()),
            Some(json!({"columns": columns, "rows": rows})),
        );
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_outputs_have_widget_payloads_not_routing_metadata() {
        assert_eq!(content_payload("| value | value |\n| --- | --- |\n| old | new |"), (None, None));
        assert_eq!(content_payload("|  | value |\n| --- | --- |\n| old | new |"), (None, None));
        let diff = "diff --git a/demo b/demo\n@@ -1 +1 @@\n-old\n+new";
        assert_eq!(
            content_payload(diff),
            (Some("diff".into()), Some(json!({"unified": diff})))
        );
        let table = content_payload("| file | status |\n| --- | --- |\n| demo | changed |");
        assert_eq!(
            table.1.unwrap()["rows"][0],
            json!({"file":"demo", "status":"changed"})
        );
        assert_eq!(
            envelope_for_action_result("git.current_branch", json!("main")),
            json!({"data":"main"})
        );
        assert_eq!(
            envelope_for_action_result("git.pull", Value::Null),
            json!({"data":null})
        );
        let rows = json!([{"file":"demo"}]);
        assert_eq!(
            envelope_for_action_result("git.status", rows.clone()),
            json!({"renderer":"table", "data":rows})
        );
    }
}
