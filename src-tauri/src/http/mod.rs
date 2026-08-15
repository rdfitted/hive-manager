pub mod error;
pub mod handlers;
pub mod routes;
pub mod state;
#[cfg(test)]
pub mod tests;
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
    let app = create_router(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}
