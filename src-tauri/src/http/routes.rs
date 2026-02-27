use axum::{
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use crate::http::state::AppState;
use crate::http::handlers::{health, sessions, inject, workers, planners, learnings, conversations, heartbeats};

pub fn create_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/health", get(health::health_check))
        .route("/api/sessions", get(sessions::list_sessions))
        // Heartbeat routes (active must be before {id} to match)
        .route("/api/sessions/active", get(heartbeats::get_active_sessions))
        .route("/api/sessions/{id}/heartbeat", post(heartbeats::post_heartbeat))
        .route("/api/sessions/{id}", get(sessions::get_session))
        .route("/api/sessions/hive", post(sessions::launch_hive))
        .route("/api/sessions/swarm", post(sessions::launch_swarm))
        .route("/api/sessions/solo", post(sessions::launch_solo))
        .route("/api/sessions/fusion", post(sessions::launch_fusion))
        .route("/api/sessions/{id}/fusion/select-winner", post(sessions::select_fusion_winner))
        .route("/api/sessions/{id}/fusion/status", get(sessions::get_fusion_status))
        .route("/api/sessions/{id}/fusion/evaluation", get(sessions::get_fusion_evaluation))
        .route("/api/sessions/{id}/stop", post(sessions::stop_session))
        .route("/api/sessions/{id}/close", post(sessions::close_session))
        // Worker routes
        .route("/api/sessions/{id}/workers", get(workers::list_workers))
        .route("/api/sessions/{id}/workers", post(workers::add_worker))
        // Planner routes (Swarm mode)
        .route("/api/sessions/{id}/planners", get(planners::list_planners))
        .route("/api/sessions/{id}/planners", post(planners::add_planner))
        // Learning routes (legacy - work when single project active)
        .route("/api/learnings", get(learnings::list_learnings))
        .route("/api/learnings", post(learnings::submit_learning))
        .route("/api/project-dna", get(learnings::get_project_dna))
        // Session-scoped learning routes (preferred - work with multiple projects)
        .route("/api/sessions/{id}/learnings", get(learnings::list_learnings_for_session))
        .route("/api/sessions/{id}/learnings", post(learnings::submit_learning_for_session))
        .route("/api/sessions/{id}/learnings/{learning_id}", delete(learnings::delete_learning_for_session))
        .route("/api/sessions/{id}/project-dna", get(learnings::get_project_dna_for_session))
        // Conversation routes
        .route("/api/sessions/{id}/conversations/{agent}", get(conversations::read_conversation))
        .route("/api/sessions/{id}/conversations/{agent}/append", post(conversations::append_conversation))
        // Injection routes
        .route("/api/sessions/{id}/inject", post(inject::operator_inject))
        .route("/api/sessions/{id}/inject/queen", post(inject::queen_inject))
        .layer(cors)
        .with_state(state)
}
