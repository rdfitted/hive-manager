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
    auth::set_api_port(listener.local_addr().map(|bound| bound.port()).unwrap_or(port));
    // Publish the token only once this launch owns the port, so a second
    // instance that fails to bind cannot overwrite the live token.
    publish_api_token(&base_dir).await;
    axum::serve(listener, app).await
}

/// Write this launch's token file, retrying transient failures. If it still
/// fails, remove the previous launch's file so file-based callers see "no
/// token" rather than a stale one. If even removal fails (for example a
/// read-only data directory), nothing in this process can change the file; the
/// error names the path so the operator can fix it. The API keeps serving
/// either way for callers that get the token from the environment (agents) or
/// the Tauri command (frontend).
#[cfg_attr(test, allow(dead_code))]
async fn publish_api_token(base_dir: &std::path::Path) {
    let outcome = publish_api_token_with(base_dir, std::time::Duration::from_millis(250), |dir| {
        auth::persist_api_token(dir, auth::api_token())
    })
    .await;
    match outcome {
        TokenPublication::Written(path) => {
            tracing::info!("HTTP API token written to {}", path.display())
        }
        TokenPublication::StaleRemoved(error) => tracing::error!(
            "Failed to write the HTTP API token file ({error}); removed the previous launch's \
             file. File-based API callers will get 401 until the next launch."
        ),
        TokenPublication::StaleKept { write, remove, path } => tracing::error!(
            "Failed to write the HTTP API token file ({write}) and to remove the stale one at {} \
             ({remove}). File-based API callers will get 401; fix the directory permissions \
             and relaunch.",
            path.display()
        ),
    }
}

#[derive(Debug)]
enum TokenPublication {
    Written(std::path::PathBuf),
    StaleRemoved(std::io::Error),
    StaleKept {
        write: std::io::Error,
        remove: std::io::Error,
        path: std::path::PathBuf,
    },
}

async fn publish_api_token_with(
    base_dir: &std::path::Path,
    backoff: std::time::Duration,
    persist: impl Fn(&std::path::Path) -> std::io::Result<std::path::PathBuf>,
) -> TokenPublication {
    const ATTEMPTS: u32 = 3;
    let mut attempt = 1;
    let write_error = loop {
        match persist(base_dir) {
            Ok(path) => return TokenPublication::Written(path),
            Err(error) if attempt < ATTEMPTS => {
                tracing::warn!("HTTP API token write attempt {attempt} failed: {error}");
                tokio::time::sleep(backoff * attempt).await;
                attempt += 1;
            }
            Err(error) => break error,
        }
    };
    let path = auth::api_token_path(base_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => TokenPublication::StaleRemoved(write_error),
        Err(remove) if remove.kind() == std::io::ErrorKind::NotFound => {
            TokenPublication::StaleRemoved(write_error)
        }
        Err(remove) => TokenPublication::StaleKept { write: write_error, remove, path },
    }
}

#[cfg(test)]
mod token_publication_tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Duration;

    fn failing() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "synthetic")
    }

    #[tokio::test]
    async fn a_transient_failure_is_retried() {
        let dir = tempfile::TempDir::new().unwrap();
        let calls = Cell::new(0);
        let outcome = publish_api_token_with(dir.path(), Duration::ZERO, |base| {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                Err(failing())
            } else {
                auth::persist_api_token(base, "fresh")
            }
        })
        .await;
        assert!(matches!(outcome, TokenPublication::Written(_)), "{outcome:?}");
        assert_eq!(calls.get(), 3);
        assert_eq!(std::fs::read_to_string(auth::api_token_path(dir.path())).unwrap(), "fresh");
    }

    #[tokio::test]
    async fn a_persistent_failure_removes_the_stale_token() {
        let dir = tempfile::TempDir::new().unwrap();
        auth::persist_api_token(dir.path(), "previous-launch").unwrap();
        let outcome =
            publish_api_token_with(dir.path(), Duration::ZERO, |_| Err(failing())).await;
        assert!(matches!(outcome, TokenPublication::StaleRemoved(_)), "{outcome:?}");
        assert!(!auth::api_token_path(dir.path()).exists());
    }

    #[tokio::test]
    async fn an_unremovable_stale_token_is_reported_with_its_path() {
        let dir = tempfile::TempDir::new().unwrap();
        // A directory at the token path cannot be removed with remove_file.
        std::fs::create_dir(auth::api_token_path(dir.path())).unwrap();
        let outcome =
            publish_api_token_with(dir.path(), Duration::ZERO, |_| Err(failing())).await;
        match outcome {
            TokenPublication::StaleKept { path, .. } => {
                assert_eq!(path, auth::api_token_path(dir.path()))
            }
            other => panic!("expected StaleKept, got {other:?}"),
        }
    }
}
