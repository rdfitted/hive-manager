use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::{
    domain::{Cell, CellType, SessionMode, Workspace, WorkspaceStrategy},
    http::{error::ApiError, state::AppState},
    session::{
        cell_status::{
            aggregate_cell_status, variant_to_cell_id, PRIMARY_CELL_ID, RESOLVER_CELL_ID,
        },
        Session, SessionType,
    },
    storage::SessionStorage,
    workspace::git,
};

use super::{validate_cell_id, validate_session_id};

pub(crate) use crate::session::cell_status::agent_in_cell;

pub(crate) fn build_cells(session: &Session, storage: &SessionStorage) -> Vec<Cell> {
    match &session.session_type {
        SessionType::Fusion { variants } | SessionType::Debate { variants }
            if !variants.is_empty() =>
        {
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

pub(crate) fn find_cell(
    session: &Session,
    storage: &SessionStorage,
    cell_id: &str,
) -> Option<Cell> {
    build_cells(session, storage)
        .into_iter()
        .find(|cell| cell.id == cell_id)
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
        status: aggregate_cell_status(session, PRIMARY_CELL_ID),
        objective: session
            .name
            .clone()
            .unwrap_or_else(|| "Primary session cell".to_string()),
        workspace: primary_workspace(session),
        agents: session
            .agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect(),
        artifacts: storage
            .load_artifact(&session.id, PRIMARY_CELL_ID)
            .ok()
            .flatten(),
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
        objective: if matches!(&session.session_type, SessionType::Debate { .. }) {
            format!("Debate debater {}", variant)
        } else {
            format!("Fusion variant {}", variant)
        },
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
        artifacts: storage
            .load_artifact(&session.id, RESOLVER_CELL_ID)
            .ok()
            .flatten(),
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

fn primary_workspace(session: &Session) -> Workspace {
    let strategy = if session.no_git {
        WorkspaceStrategy::None
    } else if matches!(&session.session_type, SessionType::Hive { .. }) {
        session.execution_policy.workspace_strategy
    } else {
        WorkspaceStrategy::SharedCell
    };

    let worktree_path = if session.no_git {
        None
    } else {
        session.worktree_path.as_ref().map(std::path::PathBuf::from)
    };
    let branch_name = session
        .worktree_branch
        .clone()
        .or_else(|| {
            worktree_path
                .as_deref()
                .and_then(|path| git::current_branch(path).ok())
        })
        .unwrap_or_else(|| {
            git::generate_branch_name(
                &session.id,
                PRIMARY_CELL_ID,
                session_mode(session),
                CellType::Hive,
            )
        });
    let is_dirty = worktree_path
        .as_deref()
        .and_then(|path| git::is_dirty(path).ok())
        .unwrap_or(false);

    Workspace {
        strategy,
        repo_path: session.project_path.clone(),
        base_branch: "unknown".to_string(),
        branch_name,
        worktree_path,
        is_dirty,
    }
}

fn session_mode(session: &Session) -> SessionMode {
    match &session.session_type {
        SessionType::Fusion { .. } => SessionMode::Fusion,
        SessionType::Debate { .. } => SessionMode::Debate,
        SessionType::Hive { .. } | SessionType::Swarm { .. } | SessionType::Solo { .. } => {
            SessionMode::Hive
        }
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
