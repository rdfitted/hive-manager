//! Pure launch planning for session orchestration.

use crate::domain::{HiveExecutionPolicy, HiveLaunchKind, WorkspaceStrategy};

/// Resolved topology used by the runtime. `launch_kind` is never `Auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiveTopologyPlan {
    pub launch_kind: HiveLaunchKind,
    pub workspace_strategy: WorkspaceStrategy,
}

impl HiveTopologyPlan {
    pub fn uses_shared_cell(self) -> bool {
        self.workspace_strategy == WorkspaceStrategy::SharedCell
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOrchestratorError {
    InvalidHiveWorkspaceStrategy(WorkspaceStrategy),
}

impl std::fmt::Display for SessionOrchestratorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHiveWorkspaceStrategy(strategy) => write!(
                formatter,
                "workspace strategy {:?} is not valid for a Hive launch",
                strategy
            ),
        }
    }
}

impl std::error::Error for SessionOrchestratorError {}

/// Stateless topology planner. Runtime side effects stay in `SessionController`;
/// launch-kind and workspace decisions live here so every entrypoint resolves
/// the same contract.
pub struct SessionOrchestrator;

impl SessionOrchestrator {
    pub fn plan_hive_launch(
        policy: &HiveExecutionPolicy,
        worker_count: usize,
        allow_no_workspace: bool,
    ) -> Result<HiveTopologyPlan, SessionOrchestratorError> {
        if policy.workspace_strategy == WorkspaceStrategy::None && !allow_no_workspace {
            return Err(SessionOrchestratorError::InvalidHiveWorkspaceStrategy(
                policy.workspace_strategy,
            ));
        }

        let launch_kind = match policy.launch_kind {
            HiveLaunchKind::Auto if worker_count == 0 => HiveLaunchKind::Solo,
            HiveLaunchKind::Auto => HiveLaunchKind::Hive,
            explicit => explicit,
        };

        Ok(HiveTopologyPlan {
            launch_kind,
            workspace_strategy: policy.workspace_strategy,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_preserves_legacy_empty_worker_sentinel() {
        let policy = HiveExecutionPolicy::default();
        assert_eq!(
            SessionOrchestrator::plan_hive_launch(&policy, 0, false)
                .unwrap()
                .launch_kind,
            HiveLaunchKind::Solo
        );
        assert_eq!(
            SessionOrchestrator::plan_hive_launch(&policy, 1, false)
                .unwrap()
                .launch_kind,
            HiveLaunchKind::Hive
        );
    }

    #[test]
    fn explicit_hive_stays_hive_with_empty_roster() {
        let policy = HiveExecutionPolicy {
            launch_kind: HiveLaunchKind::Hive,
            workspace_strategy: WorkspaceStrategy::SharedCell,
            ..HiveExecutionPolicy::default()
        };

        let plan = SessionOrchestrator::plan_hive_launch(&policy, 0, false).unwrap();
        assert_eq!(plan.launch_kind, HiveLaunchKind::Hive);
        assert!(plan.uses_shared_cell());
    }

    #[test]
    fn explicit_solo_does_not_depend_on_roster() {
        let policy = HiveExecutionPolicy {
            launch_kind: HiveLaunchKind::Solo,
            ..HiveExecutionPolicy::default()
        };
        assert_eq!(
            SessionOrchestrator::plan_hive_launch(&policy, 4, false)
                .unwrap()
                .launch_kind,
            HiveLaunchKind::Solo
        );
    }

    #[test]
    fn none_workspace_is_rejected_for_hive() {
        let policy = HiveExecutionPolicy {
            workspace_strategy: WorkspaceStrategy::None,
            ..HiveExecutionPolicy::default()
        };
        assert!(matches!(
            SessionOrchestrator::plan_hive_launch(&policy, 1, false),
            Err(SessionOrchestratorError::InvalidHiveWorkspaceStrategy(
                WorkspaceStrategy::None
            ))
        ));
    }

    #[test]
    fn none_workspace_is_allowed_for_no_git_hive_profiles() {
        let policy = HiveExecutionPolicy {
            launch_kind: HiveLaunchKind::Hive,
            workspace_strategy: WorkspaceStrategy::None,
            ..HiveExecutionPolicy::default()
        };

        let plan = SessionOrchestrator::plan_hive_launch(&policy, 2, true).unwrap();
        assert_eq!(plan.launch_kind, HiveLaunchKind::Hive);
        assert_eq!(plan.workspace_strategy, WorkspaceStrategy::None);
    }
}
