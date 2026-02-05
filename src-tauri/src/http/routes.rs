use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use crate::http::state::AppState;
use crate::http::handlers::{health, sessions, inject, workers, planners};

pub fn create_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/health", get(health::health_check))
        .route("/api/sessions", get(sessions::list_sessions))
        .route("/api/sessions/{id}", get(sessions::get_session))
        .route("/api/sessions/hive", post(sessions::launch_hive))
        .route("/api/sessions/swarm", post(sessions::launch_swarm))
        .route("/api/sessions/{id}/stop", post(sessions::stop_session))
        // Worker routes
        .route("/api/sessions/{id}/workers", get(workers::list_workers))
        .route("/api/sessions/{id}/workers", post(workers::add_worker))
        // Planner routes (Swarm mode)
        .route("/api/sessions/{id}/planners", get(planners::list_planners))
        .route("/api/sessions/{id}/planners", post(planners::add_planner))
        // Injection routes
        .route("/api/sessions/{id}/inject", post(inject::operator_inject))
        .route("/api/sessions/{id}/inject/queen", post(inject::queen_inject))
        .layer(cors)
        .with_state(state)
}
