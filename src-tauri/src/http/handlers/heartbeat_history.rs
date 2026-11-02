//! Read the existing per-agent heartbeat snapshot without exposing credentials.
use super::validate_session_id;
use crate::http::{error::ApiError, state::AppState};
use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Serialize)]
pub struct HeartbeatSnapshot {
    last_activity: String,
    status: String,
    summary: Option<String>,
}

/// GET /api/sessions/{id}/heartbeats. This is a latest-value snapshot, not an event log.
pub async fn get_heartbeats(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<BTreeMap<String, HeartbeatSnapshot>>, ApiError> {
    validate_session_id(&session_id)?;
    let controller = state.session_controller.read();
    if controller.get_session(&session_id).is_none()
        && !state
            .storage
            .session_dir(&session_id)
            .join("session.json")
            .is_file()
    {
        return Err(ApiError::not_found("Session not found"));
    }
    let snapshots = controller
        .get_heartbeat_info(&session_id)
        .into_iter()
        .map(|(id, info)| {
            (
                id,
                HeartbeatSnapshot {
                    last_activity: info.last_activity.to_rfc3339(),
                    status: info.status,
                    summary: info.summary.map(|summary| {
                        super::heartbeats::redact_credential_text(&state, &summary, None)
                    }),
                },
            )
        })
        .collect();
    Ok(Json(snapshots))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn g39_r2_heartbeat_read_redacts_live_credentials_in_summary_without_changing_storage() {
        let (_temp, state, app) = crate::http::tests_agent_events::setup();
        let id = "r2-heartbeat";
        let agent = "r2-heartbeat-worker-1";
        state.session_controller.write().insert_test_session(
            crate::http::tests::make_test_session_with_agents(id, "/repo", &[agent]),
        );
        let secret = state.pty_manager.read().mint_heartbeat_token_for_test(
            "foreign-worker",
            "foreign",
            false,
        );
        for summary in [
            format!("credential: {secret}"),
            format!("{}\t{}", &secret[..32], &secret[32..]),
        ] {
            state
                .session_controller
                .read()
                .update_heartbeat(id, agent, "working", Some(&summary))
                .unwrap();
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/sessions/{id}/heartbeats"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(
                body[agent]["summary"] == "redacted-credential",
                "heartbeat summary containing a live credential must be redacted"
            );
            assert!(!String::from_utf8_lossy(&bytes).contains(&secret));
            let stored = state.session_controller.read().get_heartbeat_info(id);
            assert!(
                stored[agent].summary.as_deref() == Some(summary.as_str()),
                "read-time redaction must not change the stored summary"
            );
        }
    }

    #[tokio::test]
    async fn heartbeat_read_api_returns_existing_data_without_tokens_and_unknown_is_404() {
        let (_temp, state, app) = crate::http::tests_agent_events::setup();
        let id = "heartbeat-read";
        let agent = "heartbeat-read-worker-1";
        state.session_controller.write().insert_test_session(
            crate::http::tests::make_test_session_with_agents(id, "/repo", &[agent]),
        );
        let secret = state
            .session_controller
            .read()
            .heartbeat_token_for_test(id, agent);
        state
            .session_controller
            .read()
            .update_heartbeat(id, agent, "working", Some("checking API"))
            .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{id}/heartbeats"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains(&secret));
        for token in [
            "heartbeat_token",
            "HIVE_AGENT_TOKEN",
            "auth_token",
            "X-Hive-Agent-Token",
        ] {
            assert!(!text.contains(token), "secret field leaked: {token}");
        }
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body[agent]["status"], "working");
        assert_eq!(body[agent]["summary"], "checking API");
        assert!(body[agent]["last_activity"].as_str().is_some());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/unknown-heartbeat/heartbeats")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!state.storage.session_dir("unknown-heartbeat").exists());
    }
}
