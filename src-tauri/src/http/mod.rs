pub mod auth;
pub mod error;
pub mod handlers;
pub mod routes;
pub mod state;
#[cfg(test)]
pub mod tests;
#[cfg(test)]
pub mod tests_agent_events;
#[cfg(test)]
pub mod tests_transport;
#[cfg(test)]
mod tests_plan_continue;
#[cfg(test)]
mod tests_terminal_continuity;
#[cfg(test)]
mod tests_wg_codegraph;
#[cfg(test)]
mod tests_wg_context;
#[cfg(test)]
mod tests_wg_plan;
#[cfg(test)]
mod tests_wg_queue;
#[cfg(test)]
mod tests_wg_retro;
#[cfg(test)]
mod tests_wg_review;
#[cfg(test)]
mod tests_wg_runtime;
#[cfg(test)]
mod tests_wg_schema;
#[cfg(test)]
mod tests_wg_state;
#[cfg(test)]
mod tests_wg_templates;
#[cfg(test)]
mod tests_wg_api;
#[cfg(test)]
mod tests_wg_roles;
#[cfg(test)]
mod tests_wg_authority;
#[cfg(test)]
mod tests_wg_verifier;

use crate::http::routes::create_router;
use crate::http::state::AppState;
use std::net::SocketAddr;
use std::sync::Arc;

#[cfg_attr(test, allow(dead_code))]
pub async fn serve(state: Arc<AppState>, port: u16) -> Result<(), std::io::Error> {
    let base_dir = state.storage.base_dir().clone();
    let app = create_router(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // Publish the token only once this launch owns the port, so a second
    // instance that fails to bind cannot overwrite the live token.
    publish_api_token(&base_dir).await;
    axum::serve(listener, app).await
}

/// Write this launch's token file, retrying transient failures. If it still
/// fails, remove the previous launch's file so file-based callers see "no
/// token" rather than a stale one; the API keeps serving for callers that get
/// the token from the environment (agents) or the Tauri command (frontend).
#[cfg_attr(test, allow(dead_code))]
async fn publish_api_token(base_dir: &std::path::Path) {
    const ATTEMPTS: u64 = 3;
    for attempt in 1..=ATTEMPTS {
        match auth::persist_api_token(base_dir, auth::api_token()) {
            Ok(path) => {
                tracing::info!("HTTP API token written to {}", path.display());
                return;
            }
            Err(error) if attempt < ATTEMPTS => {
                tracing::warn!("HTTP API token write attempt {attempt} failed: {error}");
                tokio::time::sleep(std::time::Duration::from_millis(250 * attempt)).await;
            }
            Err(error) => {
                tracing::error!("Failed to write the HTTP API token file: {error}");
                if let Err(remove) = std::fs::remove_file(auth::api_token_path(base_dir)) {
                    if remove.kind() != std::io::ErrorKind::NotFound {
                        tracing::error!("Failed to remove the stale HTTP API token file: {remove}");
                    }
                }
            }
        }
    }
}
