//! Event handlers: query and SSE streaming endpoints.

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::stream::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::domain::event::Event as DomainEvent;
use crate::http::error::ApiError;
use crate::http::handlers::validate_session_id;
use crate::http::state::AppState;

/// GET /api/sessions/{id}/events
/// Query persisted events for a session from JSONL storage.
pub async fn get_events(
    State(state): State<std::sync::Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<DomainEvent>>, ApiError> {
    validate_session_id(&session_id)?;

    // Use the writer's root, including files persisted before this endpoint fix.
    let events_file = state.event_bus.data_dir().join(&session_id).join("events.jsonl");

    if !events_file.exists() {
        return Ok(Json(Vec::new()));
    }

    let contents = tokio::fs::read_to_string(&events_file)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to read events file: {e}")))?;

    let events: Vec<DomainEvent> = contents
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    Ok(Json(events))
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn get_events_reads_bus_persistence_and_existing_files() {
        let (_temp, state, app) = crate::http::tests_agent_events::setup();
        let event = crate::domain::event::Event {
            id: "persisted-event".into(),
            session_id: "events-session".into(),
            cell_id: None,
            agent_id: None,
            event_type: crate::domain::event::EventType::SessionCreated,
            timestamp: chrono::Utc::now(),
            payload: serde_json::json!({"message": "existing event"}),
            severity: crate::domain::event::Severity::Info,
        };
        let existing_dir = state.event_bus.data_dir().join("events-session");
        std::fs::create_dir_all(&existing_dir).unwrap();
        let mut historical = event.clone();
        historical.id = "historical-event".into();
        std::fs::write(existing_dir.join("events.jsonl"),
            format!("{}\n", serde_json::to_string(&historical).unwrap())).unwrap();
        state.event_bus.publish(event.clone()).await.unwrap();
        assert!(state.event_bus.data_dir().join("events-session/events.jsonl").exists());
        assert!(!state.storage.session_dir("events-session").join("events.jsonl").exists());
        let response = app.oneshot(Request::builder()
            .uri("/api/sessions/events-session/events").body(Body::empty()).unwrap())
            .await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let events: Vec<crate::domain::event::Event> = serde_json::from_slice(&body).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, historical.id);
        assert_eq!(events[1].id, event.id);
    }
}

/// GET /api/sessions/{id}/stream
/// SSE endpoint for real-time event streaming.
pub async fn stream_events(
    State(state): State<std::sync::Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    validate_session_id(&session_id)?;

    let event_bus = Arc::clone(&state.event_bus);
    let session_id_filter = session_id.clone();

    // Subscribe to all events and filter by session_id
    let receiver = event_bus.subscribe();
    let stream = BroadcastStream::new(receiver)
        .filter_map(move |result| {
            let sid = session_id_filter.clone();
            async move {
                match result {
                    Ok(event) if event.session_id == sid => {
                        // Serialize event to JSON for SSE data field
                        let json = serde_json::to_string(&event).ok()?;
                        let event_type = serde_json::to_string(&event.event_type)
                            .ok()?
                            .trim_matches('"')
                            .to_string();
                        
                        Some(Ok(Event::default()
                            .event(event_type)
                            .data(json)))
                    }
                    Ok(_) => None, // Filtered out (different session)
                    Err(BroadcastStreamRecvError::Lagged(n)) => {
                        // Emit synthetic SSE frame for lagged clients
                        // CONTRACT: frontend expects event name 'lagged' and JSON {"dropped": N}
                        tracing::warn!("SSE client lagged, dropped {} events", n);
                        Some(Ok(Event::default()
                            .event("lagged")
                            .data(format!(r#"{{"dropped":{}}}"#, n))))
                    }
                }
            }
        });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
