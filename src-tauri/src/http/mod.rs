pub mod error;
pub mod routes;
pub mod state;
pub mod handlers;
#[cfg(test)]
pub mod tests;

use crate::http::routes::create_router;
use crate::http::state::AppState;
use std::net::SocketAddr;
use std::sync::Arc;

pub async fn serve(state: Arc<AppState>, port: u16) -> Result<(), std::io::Error> {
    let app = create_router(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}
