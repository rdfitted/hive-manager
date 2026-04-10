use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::{
    domain::{Cell, CellStatus, CellType, SessionMode, Workspace, WorkspaceStrategy},
    http::{error::ApiError, state::AppState},
    pty::{AgentRole as PtyAgentRole, AgentStatus as PtyAgentStatus},
    session::{AgentInfo, Session, SessionState, SessionType},
    storage::SessionStorage,
    workspace::git,
};

use super::{validate_cell_id, validate_session_id};

const PRIMARY_CELL_ID: &str = "primary";
const RESOLVER_CELL_ID: &str = "resolver";

pub(crate) fn build_cells(session: &Session, storage: &SessionStorage) -> Vec<Cell> {
    match &session.session_type {
        SessionType::Fusion { variants } if !variants.is_empty() => {
            let mut cells: Vec<Cell> = variants
                .iter()
                .map(|variant| build_fusion_cell(session, storage, variant))
                .collect();
            // Add resolver cell for non-variant agents (judge, planner, evaluator, QA workers)
            cells.push(build_resolver_cell(session, storage));
            cells
        }
        _ => vec![build_primary_cell(session, storage)],
    }
}

pub(crate) fn find_cell(session: &Session, storage: &SessionStorage, cell_id: &str) -> Option<Cell> {
    build_cells(session, storage)
        .into_iter()
        .find(|cell| cell.id == cell_id)
}

pub(crate) fn agent_in_cell(session: &Session, cell_id: &str, agent: &AgentInfo) -> bool {
    match &session.session_type {
        SessionType::Fusion { .. } if cell_id == RESOLVER_CELL_ID => {
            // Resolver cell contains non-variant agents
            !matches!(&agent.role, PtyAgentRole::Fusion { .. })
        }
        SessionType::Fusion { .. } if cell_id != PRIMARY_CELL_ID => matches!(
            &agent.role,
            PtyAgentRole::Fusion { variant } if variant_to_cell_id(variant) == cell_id
        ),
        _ => true,
    }
}

fn build_primary_cell(session: &Session, storage: &SessionStorage) -> Cell {
    Cell {
        id: PRIMARY_CELL_ID.to_string(),
        session_id: session.id.clone(),
        cell_type: CellType::Hive,
        name: session
            .name
            .clone()
            .unwrap_or_else(|| "Primary".to_string()),
        status: session_state_to_cell_status(&session.state),
        objective: session
            .name
            .clone()
            .unwrap_or_else(|| "Primary session cell".to_string()),
        workspace: synthetic_workspace(
            session,
            WorkspaceStrategy::SharedCell,
            CellType::Hive,
            PRIMARY_CELL_ID,
        ),
        agents: session.agents.iter().map(|agent| agent.id.clone()).collect(),
        artifacts: storage.load_artifact(&session.id, PRIMARY_CELL_ID).ok().flatten(),
        events: vec![],
        depends_on: vec![],
    }
}

fn build_fusion_cell(session: &Session, storage: &SessionStorage, variant: &str) -> Cell {
    let cell_id = variant_to_cell_id(variant);
    let agents: Vec<String> = session
        .agents
        .iter()
        .filter(|agent| agent_in_cell(session, &cell_id, agent))
        .map(|agent| agent.id.clone())
        .collect();

    let status = aggregate_cell_status(session, &cell_id);

    Cell {
        id: cell_id.clone(),
        session_id: session.id.clone(),
        cell_type: CellType::Hive,
        name: variant.to_string(),
        status,
        objective: format!("Fusion variant {}", variant),
        workspace: synthetic_workspace(
            session,
            WorkspaceStrategy::IsolatedCell,
            CellType::Hive,
            variant,
        ),
        agents,
        artifacts: storage.load_artifact(&session.id, &cell_id).ok().flatten(),
        events: vec![],
        depends_on: vec![],
    }
}

fn build_resolver_cell(session: &Session, storage: &SessionStorage) -> Cell {
    let agents: Vec<String> = session
        .agents
        .iter()
        .filter(|agent| agent_in_cell(session, RESOLVER_CELL_ID, agent))
        .map(|agent| agent.id.clone())
        .collect();

    let status = aggregate_cell_status(session, RESOLVER_CELL_ID);

    Cell {
        id: RESOLVER_CELL_ID.to_string(),
        session_id: session.id.clone(),
        cell_type: CellType::Resolver,
        name: "Resolver".to_string(),
        status,
        objective: "System agents (judge, planner, evaluator, QA workers)".to_string(),
        workspace: synthetic_workspace(
            session,
            WorkspaceStrategy::SharedCell,
            CellType::Resolver,
            RESOLVER_CELL_ID,
        ),
        agents,
        artifacts: storage.load_artifact(&session.id, RESOLVER_CELL_ID).ok().flatten(),
        events: vec![],
        depends_on: vec![],
    }
}

fn synthetic_workspace(
    session: &Session,
    strategy: WorkspaceStrategy,
    cell_type: CellType,
    cell_name: &str,
) -> Workspace {
    Workspace {
        strategy,
        repo_path: session.project_path.clone(),
        base_branch: "unknown".to_string(),
        branch_name: git::generate_branch_name(
            &session.id,
            cell_name,
            session_mode(session),
            cell_type,
        ),
        worktree_path: None,
        is_dirty: false,
    }
}

fn session_mode(session: &Session) -> SessionMode {
    match session.session_type {
        SessionType::Fusion { .. } => SessionMode::Fusion,
        SessionType::Hive { .. } | SessionType::Swarm { .. } | SessionType::Solo { .. } => {
            SessionMode::Hive
        }
    }
}

fn variant_to_cell_id(variant: &str) -> String {
    let normalized = variant
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    let trimmed = normalized.trim_matches('-');
    if trimmed.is_empty() {
        PRIMARY_CELL_ID.to_string()
    } else {
        trimmed.to_string()
    }
}

fn session_state_to_cell_status(state: &SessionState) -> CellStatus {
    match state {
        SessionState::Planning | SessionState::PlanReady => CellStatus::Queued,
        SessionState::Starting
        | SessionState::SpawningWorker(_)
        | SessionState::SpawningPlanner(_)
        | SessionState::SpawningFusionVariant(_)
        | SessionState::SpawningJudge
        | SessionState::SpawningEvaluator => CellStatus::Launching,
        SessionState::WaitingForWorker(_)
        | SessionState::WaitingForPlanner(_)
        | SessionState::WaitingForFusionVariants
        | SessionState::Judging
        | SessionState::MergingWinner
        | SessionState::QaInProgress { .. }
        | SessionState::Running => CellStatus::Running,
        SessionState::AwaitingVerdictSelection | SessionState::Paused => CellStatus::WaitingInput,
        SessionState::QaPassed | SessionState::Completed | SessionState::Closed => CellStatus::Completed,
        SessionState::QaFailed { .. } | SessionState::QaMaxRetriesExceeded | SessionState::Failed(_) => {
            CellStatus::Failed
        }
        SessionState::Closing => CellStatus::Summarizing,
    }
}

fn agent_status_to_cell_status(status: &PtyAgentStatus) -> CellStatus {
    match status {
        PtyAgentStatus::Starting => CellStatus::Launching,
        PtyAgentStatus::Running | PtyAgentStatus::Idle => CellStatus::Running,
        PtyAgentStatus::WaitingForInput(_) => CellStatus::WaitingInput,
        PtyAgentStatus::Completed => CellStatus::Completed,
        PtyAgentStatus::Error(_) => CellStatus::Failed,
    }
}

fn aggregate_cell_status(session: &Session, cell_id: &str) -> CellStatus {
    let agent_statuses = session
        .agents
        .iter()
        .filter(|agent| agent_in_cell(session, cell_id, agent))
        .map(|agent| agent_status_to_cell_status(&agent.status))
        .collect::<Vec<_>>();

    if agent_statuses.iter().any(|status| *status == CellStatus::Running) {
        CellStatus::Running
    } else if !agent_statuses.is_empty()
        && agent_statuses.iter().all(|status| *status == CellStatus::Completed)
    {
        CellStatus::Completed
    } else if agent_statuses.iter().any(|status| *status == CellStatus::Failed) {
        CellStatus::Failed
    } else if agent_statuses
        .iter()
        .any(|status| *status == CellStatus::WaitingInput)
    {
        CellStatus::WaitingInput
    } else {
        session_state_to_cell_status(&session.state)
    }
}

pub async fn list_cells(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<Cell>>, ApiError> {
    validate_session_id(&session_id)?;

    let controller = state.session_controller.read();
    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    Ok(Json(build_cells(&session, &state.storage)))
}

pub async fn get_cell(
    State(state): State<Arc<AppState>>,
    Path((session_id, cell_id)): Path<(String, String)>,
) -> Result<Json<Cell>, ApiError> {
    validate_session_id(&session_id)?;
    validate_cell_id(&cell_id)?;

    let controller = state.session_controller.read();
    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    let cell = find_cell(&session, &state.storage, &cell_id)
        .ok_or_else(|| ApiError::not_found(format!("Cell {} not found", cell_id)))?;

    Ok(Json(cell))
}

pub async fn stop_cell(
    State(state): State<Arc<AppState>>,
    Path((session_id, cell_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    validate_session_id(&session_id)?;
    validate_cell_id(&cell_id)?;

    {
        let controller = state.session_controller.read();
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
        if find_cell(&session, &state.storage, &cell_id).is_none() {
            return Err(ApiError::not_found(format!("Cell {} not found", cell_id)));
        }
    }

    Err(ApiError::bad_request(
        "Stopping individual cells is not yet supported",
    ))
}
