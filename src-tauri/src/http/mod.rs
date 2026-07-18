pub mod error;
pub mod handlers;
pub mod routes;
pub mod state;
#[cfg(test)]
pub mod tests;

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
