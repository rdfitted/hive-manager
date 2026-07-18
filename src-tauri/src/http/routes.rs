use crate::http::handlers::{
    actions, agents, application_state, artifacts, cells, conversations, evaluator, events, health,
    heartbeats, inject, knowledge, learnings, planners, queue, resolver, session_files, sessions,
    templates, workers,
};
use crate::http::state::AppState;
use crate::cli::health as cli_health;
use axum::{
    body::Body,
    http::{header::ORIGIN, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

const ALLOWED_BROWSER_ORIGINS: &[&str] = &[
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
    "http://localhost:1420",
];

fn is_allowed_browser_origin(origin: &HeaderValue) -> bool {
    ALLOWED_BROWSER_ORIGINS
        .iter()
        .any(|allowed| origin.as_bytes() == allowed.as_bytes())
}

async fn reject_disallowed_browser_origin(request: Request<Body>, next: Next) -> Response {
    if request
        .headers()
        .get(ORIGIN)
        .is_some_and(|origin| !is_allowed_browser_origin(origin))
    {
        return StatusCode::FORBIDDEN.into_response();
    }

    next.run(request).await
}

pub fn create_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            is_allowed_browser_origin(origin)
        }))
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/health", get(health::health_check))
        .route("/api/cli-health", get(cli_health::get_cli_health_http))
        // Unified action registry surface (the future agent/MCP entrypoint).
        // GET lists every action + schema; POST dispatches any action (caller=Http).
        .route("/api/actions", get(actions::list_actions))
        .route("/api/actions/{name}", post(actions::dispatch_action))
        .route(
            "/api/sessions",
            get(sessions::list_sessions).post(sessions::create_session),
        )
        // Heartbeat routes (active must be before {id} to match)
        .route("/api/sessions/active", get(heartbeats::get_active_sessions))
        .route(
            "/api/sessions/{id}/heartbeat",
            post(heartbeats::post_heartbeat),
        )
        .route(
            "/api/sessions/{id}",
            get(sessions::get_session)
                .patch(sessions::update_session)
                .delete(sessions::stop_session),
        )
        .route("/api/sessions/{id}/launch", post(sessions::launch_session))
        .route("/api/sessions/hive", post(sessions::launch_hive))
        .route("/api/sessions/swarm", post(sessions::launch_swarm))
        .route("/api/sessions/solo", post(sessions::launch_solo))
        .route("/api/sessions/fusion", post(sessions::launch_fusion))
        .route("/api/sessions/debate", post(sessions::launch_debate))
        .route(
            "/api/sessions/{id}/fusion/select-winner",
            post(sessions::select_fusion_winner),
        )
        .route(
            "/api/sessions/{id}/fusion/status",
            get(sessions::get_fusion_status),
        )
        .route(
            "/api/sessions/{id}/fusion/evaluation",
            get(sessions::get_fusion_evaluation),
        )
        .route(
            "/api/sessions/{id}/debate/status",
            get(sessions::get_debate_status),
        )
        .route(
            "/api/sessions/{id}/debate/evaluation",
            get(sessions::get_debate_evaluation),
        )
        .route(
            "/api/sessions/{id}/resolver",
            get(resolver::get_resolver_output),
        )
        .route(
            "/api/sessions/{id}/resolver/launch",
            post(resolver::launch_resolver),
        )
        .route("/api/sessions/{id}/stop", post(sessions::stop_session))
        .route("/api/sessions/{id}/close", post(sessions::close_session))
        .route(
            "/api/sessions/{id}/complete",
            post(sessions::complete_session),
        )
        // Worker routes
        .route("/api/sessions/{id}/workers", get(workers::list_workers))
        .route("/api/sessions/{id}/workers", post(workers::add_worker))
        // Read-only session artifact browser
        .route(
            "/api/sessions/{id}/files",
            get(session_files::list_session_files),
        )
        .route(
            "/api/sessions/{id}/files/content",
            get(session_files::read_session_file),
        )
        // Durable run-queue snapshot (#126)
        .route("/api/sessions/{id}/queue", get(queue::get_queue))
        // Evaluator routes
        .route(
            "/api/sessions/{id}/evaluators",
            get(evaluator::list_evaluators),
        )
        .route(
            "/api/sessions/{id}/evaluators",
            post(evaluator::add_evaluator),
        )
        .route(
            "/api/sessions/{id}/qa-workers",
            post(evaluator::add_qa_worker),
        )
        .route(
            "/api/sessions/{id}/auth/dev-login",
            get(evaluator::dev_login),
        )
        .route(
            "/api/sessions/{id}/qa/verdict",
            post(evaluator::post_verdict),
        )
        .route(
            "/api/sessions/{id}/qa/force-pass",
            post(evaluator::force_pass),
        )
        .route(
            "/api/sessions/{id}/qa/force-fail",
            post(evaluator::force_fail),
        )
        // Prince remediation verdict (self-certified after the fix team resolves QA findings)
        .route(
            "/api/sessions/{id}/prince/verdict",
            post(evaluator::post_prince_verdict),
        )
        // Planner routes (Swarm mode)
        .route("/api/sessions/{id}/planners", get(planners::list_planners))
        .route("/api/sessions/{id}/planners", post(planners::add_planner))
        // Cell / agent / artifact routes
        .route("/api/sessions/{id}/cells", get(cells::list_cells))
        .route(
            "/api/sessions/{id}/cells/{cid}",
            get(cells::get_cell).delete(cells::stop_cell),
        )
        .route(
            "/api/sessions/{id}/cells/{cid}/agents",
            get(agents::list_agents_in_cell),
        )
        .route(
            "/api/sessions/{id}/agents/{aid}",
            delete(agents::stop_agent),
        )
        .route(
            "/api/sessions/{id}/agents/{aid}/input",
            post(agents::send_agent_input),
        )
        .route(
            "/api/sessions/{id}/cells/{cid}/artifacts",
            get(artifacts::list_artifacts).post(artifacts::post_artifact),
        )
        .route(
            "/api/templates",
            get(templates::list_templates).post(templates::create_template),
        )
        .route(
            "/api/templates/{id}",
            get(templates::get_template).delete(templates::delete_template),
        )
        // Learning routes (legacy - work when single project active)
        .route("/api/learnings", get(learnings::list_learnings))
        .route("/api/learnings", post(learnings::submit_learning))
        .route("/api/project-dna", get(learnings::get_project_dna))
        // Read-only institutional knowledge graph + id-based markdown preview.
        .route("/api/knowledge/graph", get(knowledge::get_knowledge_graph))
        .route("/api/knowledge/page", get(knowledge::get_knowledge_page))
        // Session-scoped learning routes (preferred - work with multiple projects)
        .route(
            "/api/sessions/{id}/learnings",
            get(learnings::list_learnings_for_session),
        )
        .route(
            "/api/sessions/{id}/learnings",
            post(learnings::submit_learning_for_session),
        )
        .route(
            "/api/sessions/{id}/learnings/{learning_id}",
            delete(learnings::delete_learning_for_session),
        )
        .route(
            "/api/sessions/{id}/project-dna",
            get(learnings::get_project_dna_for_session),
        )
        // Conversation routes
        .route(
            "/api/sessions/{id}/conversations/{agent}",
            get(conversations::read_conversation),
        )
        .route(
            "/api/sessions/{id}/conversations/{agent}/append",
            post(conversations::append_conversation),
        )
        // Event routes
        .route("/api/sessions/{id}/events", get(events::get_events))
        .route("/api/sessions/{id}/stream", get(events::stream_events))
        // Run journal + ledger (#125): per-step status for a resumable run
        .route(
            "/api/sessions/{id}/run-journal",
            get(sessions::get_run_journal),
        )
        // Application-state routes (SQLite-backed nav/UI state + watermark polling)
        .route(
            "/api/sessions/{id}/application-state",
            get(application_state::get_application_state)
                .post(application_state::write_application_state),
        )
        .route(
            "/api/sessions/{id}/application-state/poll",
            get(application_state::poll_application_state),
        )
        // One-shot atomic read-and-delete (#128 Ctrl+I pending_selection_context).
        .route(
            "/api/sessions/{id}/application-state/take",
            post(application_state::take_application_state),
        )
        // Injection routes
        .route("/api/sessions/{id}/inject", post(inject::operator_inject))
        .route(
            "/api/sessions/{id}/inject/queen",
            post(inject::queen_inject),
        )
        .route(
            "/api/sessions/{id}/inject/evaluator",
            post(inject::evaluator_inject),
        )
        .layer(cors)
        .layer(middleware::from_fn(reject_disallowed_browser_origin))
        .with_state(state)
}
