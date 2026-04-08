use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ArtifactBundle {
    pub summary: Option<String>,
    pub changed_files: Vec<String>,
    pub commits: Vec<String>,
    pub branch: String,
    pub test_results: Option<serde_json::Value>,
    pub diff_summary: Option<String>,
    pub unresolved_issues: Vec<String>,
    pub confidence: Option<f32>,
    pub recommended_next_step: Option<String>,
}
