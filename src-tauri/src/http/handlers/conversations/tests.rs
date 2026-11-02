use axum::{
    body::Body,
    http::{HeaderValue, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::http::{state::AppState, tests::make_test_session};

fn known_session(state: &AppState, id: &str) {
    state
        .session_controller
        .read()
        .insert_test_session(make_test_session(id, "/repo"));
}

async fn append(
    app: &Router,
    session: &str,
    target: &str,
    from: &str,
    content: &str,
    tokens: &[HeaderValue],
) -> axum::response::Response {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/sessions/{session}/conversations/{target}/append"
        ))
        .header("content-type", "application/json");
    for token in tokens {
        request = request.header("X-Hive-Agent-Token", token);
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(
                    json!({"from":from,"content":content}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn read_json(app: &Router, path: &str) -> Value {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn g34_conversation_rejects_invalid_duplicate_malformed_and_foreign_tokens() {
    let (_temp, state, app) = crate::http::tests_agent_events::setup();
    let id = "conversation-auth";
    known_session(&state, id);
    let owner = format!("{id}-worker-1");
    let secret = state
        .pty_manager
        .read()
        .mint_heartbeat_token_for_test(&owner, id, false);
    let foreign =
        state
            .pty_manager
            .read()
            .mint_heartbeat_token_for_test("other-worker-1", "other", false);
    let wrong = uuid::Uuid::new_v4().simple().to_string().repeat(2);
    for tokens in [
        vec![HeaderValue::from_str(&wrong).unwrap()],
        vec![
            HeaderValue::from_str(&secret).unwrap(),
            HeaderValue::from_str(&secret).unwrap(),
        ],
        vec![HeaderValue::from_static(""), HeaderValue::from_static("")],
        vec![HeaderValue::from_bytes(&[0xff]).unwrap()],
        vec![HeaderValue::from_str(&foreign).unwrap()],
    ] {
        let response = append(&app, id, "queen", "worker-1", "rejected", &tokens).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "invalid credentials must be rejected"
        );
    }
    assert!(!state.storage.session_dir(id).join("conversations").exists());
    let events = read_json(&app, &format!("/api/sessions/{id}/events")).await;
    assert_eq!(events.as_array().unwrap().len(), 5);
    assert!(events
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["payload"]["kind"] == "conversation_rejected"));
    let text = events.to_string();
    assert!(!text.contains(&secret));
    assert!(!text.contains(&wrong));
    assert!(!text.contains(&foreign));
}

#[tokio::test]
async fn g34_conversation_owner_matches_canonical_sender_and_can_address_other_agents() {
    let (_temp, state, app) = crate::http::tests_agent_events::setup();
    let id = "conversation-owner";
    known_session(&state, id);
    let owner = format!("{id}-worker-1");
    let secret = state
        .pty_manager
        .read()
        .mint_heartbeat_token_for_test(&owner, id, false);
    let headers = [HeaderValue::from_str(&secret).unwrap()];
    for from in ["worker-1", owner.as_str()] {
        assert_eq!(
            append(&app, id, "queen", from, "hello queen", &headers)
                .await
                .status(),
            StatusCode::CREATED
        );
    }
    for from in ["worker-2", "queen", "operator"] {
        assert_eq!(
            append(&app, id, "shared", from, "forged", &headers)
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "token owner must match sender"
        );
    }
    let messages = read_json(&app, &format!("/api/sessions/{id}/conversations/queen")).await;
    assert_eq!(messages["messages"].as_array().unwrap().len(), 2);
    assert_eq!(messages["messages"][0]["from"], "worker-1");
    assert_eq!(messages["messages"][1]["from"], owner);
    assert!(!state
        .storage
        .session_dir(id)
        .join("conversations/shared.md")
        .exists());
}

#[tokio::test]
async fn g34_conversation_absent_empty_and_operator_sender_are_accepted_and_audited() {
    let (_temp, state, app) = crate::http::tests_agent_events::setup();
    let id = "conversation-legacy";
    known_session(&state, id);
    // Same tokenless headers/body as stores/conversations.ts and ConversationViewer.
    for (from, tokens) in [
        ("operator", vec![]),
        ("worker-1", vec![HeaderValue::from_static("")]),
        ("queen", vec![HeaderValue::from_static("   ")]),
    ] {
        assert_eq!(
            append(&app, id, "shared", from, "legacy message", &tokens)
                .await
                .status(),
            StatusCode::CREATED
        );
    }
    let events = read_json(&app, &format!("/api/sessions/{id}/events")).await;
    let audits: Vec<_> = events
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["payload"]["kind"] == "conversation_unauthenticated")
        .collect();
    assert_eq!(
        audits.len(),
        3,
        "each unauthenticated append must be audited"
    );
    for event in audits {
        assert_eq!(event["event_type"], "artifact_updated");
        assert!(event["agent_id"].is_null());
        assert!(event["payload"]["caller_agent_id"].is_null());
        assert!(event["payload"]["resolved_agent_id"].is_null());
        assert_eq!(event["payload"]["reason"], "verify_when_present");
    }
    let messages = read_json(&app, &format!("/api/sessions/{id}/conversations/shared")).await;
    assert_eq!(messages["messages"].as_array().unwrap().len(), 3);
    assert_eq!(messages["messages"][0]["from"], "operator");
}

#[tokio::test]
async fn g39_conversation_credentials_never_reach_audit_history_or_live_messages() {
    let (_temp, state, app) = crate::http::tests_agent_events::setup();
    let id = "conversation-secrets";
    known_session(&state, id);
    let owner = format!("{id}-worker-1");
    let secret = state
        .pty_manager
        .read()
        .mint_heartbeat_token_for_test(&owner, id, false);
    let headers = [HeaderValue::from_str(&secret).unwrap()];
    let mut events = state.event_bus.subscribe_session(id.into());
    for (target, from, content) in [
        ("queen", "worker-1", format!("credential: {secret}")),
        (
            "queen",
            "worker-1",
            format!("{}\u{1}{}", &secret[..32], &secret[32..]),
        ),
        (
            "queen",
            secret.as_str(),
            "sender contains credential".into(),
        ),
        (
            secret.as_str(),
            "worker-1",
            "target contains credential".into(),
        ),
    ] {
        let response = append(&app, id, target, from, &content, &headers).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(&secret));
        let event = events.recv().await.unwrap();
        assert_eq!(event.payload["kind"], "conversation_rejected");
        assert_eq!(event.payload["reason"], "credential_in_payload");
        assert!(
            !serde_json::to_string(&event).unwrap().contains(&secret),
            "audit must redact credential-bearing identity"
        );
    }
    // An ambiguous second header must not leak through a forged sender either.
    let second = uuid::Uuid::new_v4().simple().to_string().repeat(2);
    assert_eq!(
        append(
            &app,
            id,
            "queen",
            &second,
            "duplicate",
            &[
                HeaderValue::from_str(&secret).unwrap(),
                HeaderValue::from_str(&second).unwrap()
            ]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let event = events.recv().await.unwrap();
    assert!(!serde_json::to_string(&event).unwrap().contains(&second));
    let audit = read_json(&app, &format!("/api/sessions/{id}/events"))
        .await
        .to_string();
    assert!(!audit.contains(&secret));
    assert!(!audit.contains(&second));
    let history = read_json(&app, &format!("/api/sessions/{id}/conversations/queen")).await;
    assert!(history["messages"].as_array().unwrap().is_empty());
    assert!(!state.storage.session_dir(id).join("conversations").exists());
}
