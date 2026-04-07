use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::artifact::ArtifactBundle;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub objective: String,
    pub project_path: PathBuf,
    pub mode: SessionMode,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub cells: Vec<String>,
    pub launch_config: LaunchConfig,
    pub artifacts: Vec<ArtifactBundle>,
    pub events: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Hive,
    Fusion,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LaunchConfig {
    pub plan_source: Option<String>,
    pub default_cli: String,
    pub default_model: Option<String>,
    pub worker_count: u16,
    pub variant_count: Option<u16>,
    pub with_planning: bool,
    pub with_evaluator: bool,
    pub smoke_test: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Drafting,
    Preparing,
    Launching,
    Active,
    Resolving,
    Completed,
    PartialFailure,
    Failed,
    Cancelled,
}
