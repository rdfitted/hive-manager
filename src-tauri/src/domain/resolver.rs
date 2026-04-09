use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ResolverOutput {
    pub selected_candidate: String,
    pub rationale: String,
    pub tradeoffs: Vec<String>,
    pub hybrid_integration_plan: Option<String>,
    pub final_recommendation: Option<String>,
}
