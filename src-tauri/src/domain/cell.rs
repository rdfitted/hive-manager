use serde::{Deserialize, Serialize};

use crate::domain::artifact::ArtifactBundle;
use crate::domain::workspace::Workspace;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Cell {
    pub id: String,
    pub session_id: String,
    pub cell_type: CellType,
    pub name: String,
    pub status: CellStatus,
    pub objective: String,
    pub workspace: Workspace,
    pub agents: Vec<String>,
    pub artifacts: Option<ArtifactBundle>,
    pub events: Vec<String>,
    pub depends_on: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CellType {
    Hive,
    Resolver,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CellStatus {
    Queued,
    Preparing,
    Launching,
    Running,
    Summarizing,
    Completed,
    WaitingInput,
    Failed,
    Killed,
}
