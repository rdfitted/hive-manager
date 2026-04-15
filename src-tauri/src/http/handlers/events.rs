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

    // Read events from JSONL file in APPDATA session directory
    let events_file = state.storage.session_dir(&session_id).join("events.jsonl");

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
