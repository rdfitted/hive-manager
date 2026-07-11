use crate::domain::{
    CapabilityCard, CapabilitySupport, DelegationPolicy, NativeDelegationMode, WorkspaceStrategy,
};
use crate::pty::AgentConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContractRole {
    MasterPlanner,
    Queen,
    Principal,
    Researcher,
}

impl ContractRole {
    fn label(self) -> &'static str {
        match self {
            Self::MasterPlanner => "Master Planner",
            Self::Queen => "Queen",
            Self::Principal => "Coding Principal",
            Self::Researcher => "Researcher",
        }
    }
}

pub(crate) struct AssignmentSpec<'a> {
    pub objective: &'a str,
    pub access: &'a str,
    pub owned_scope: &'a str,
    pub authoritative_input: &'a str,
    pub deliverables: &'a [&'a str],
    pub validation: &'a [&'a str],
    pub stop_conditions: &'a [&'a str],
}

pub(crate) fn render_role_kernel(role: ContractRole) -> String {
    let authority = match role {
        ContractRole::MasterPlanner => {
            "Turn the operator's objective into one build-ready execution contract. Investigate and plan; do not implement production code."
        }
        ContractRole::Queen => {
            "Own intent, topology, assignment, integration, verification gates, and final synthesis. Delegate implementation; do not become a coding worker."
        }
        ContractRole::Principal => {
            "Own one coherent implementation workstream end to end. Stay inside the assignment, integrate native-child results, and return tested evidence."
        }
        ContractRole::Researcher => {
            "Investigate the assigned question read-only and return concise, cited findings. Do not modify the project or its git state."
        }
    };

    format!("## Role Kernel\n\n**{}:** {}", role.label(), authority)
}

pub(crate) fn render_capability_card(
    config: &AgentConfig,
    role: ContractRole,
    card: &CapabilityCard,
    policy: &DelegationPolicy,
    workspace_strategy: &WorkspaceStrategy,
    delegation_authorized: bool,
) -> String {
    let model = config.model.as_deref().unwrap_or("harness default");
    let flags = serde_json::to_string(&config.flags).unwrap_or_else(|_| "[]".to_string());
    let support = match card.native_delegation {
        CapabilitySupport::Supported => "supported",
        CapabilitySupport::Unsupported => "unsupported",
        CapabilitySupport::Unknown => "unknown",
    };
    let policy_name = match policy.mode {
        NativeDelegationMode::Disabled => "disabled",
        NativeDelegationMode::Auto => "auto",
        NativeDelegationMode::Encouraged => "encouraged",
    };
    let children = policy
        .max_children
        .map(|value| value.to_string())
        .unwrap_or_else(|| "harness default".to_string());
    let depth = policy
        .max_depth
        .map(|value| value.to_string())
        .unwrap_or_else(|| "harness default".to_string());
    let workspace = match workspace_strategy {
        WorkspaceStrategy::SharedCell => "shared Hive Cell worktree",
        WorkspaceStrategy::IsolatedCell => "isolated managed-agent worktree",
        WorkspaceStrategy::None => "current project checkout",
    };

    format!(
        "## Capability Card\n\n\
- Role: {role}\n\
- Harness: `{cli}`\n\
- Model: `{model}`\n\
- Flags: `{flags}`\n\
- Native delegation support (adapter profile, not a runtime probe): {support}\n\
- Operator policy: {policy_name}\n\
- Native delegation authorized: {allowed}\n\
- Native child guidance: max children {children}; max depth {depth}\n\
- Workspace: {workspace}; native children inherit the parent workspace and assignment\n\
- Visibility: native children are harness-managed; they are not Hive Manager Workers, Cells, queue rows, or separate worktrees",
        role = role.label(),
        cli = config.cli,
        model = model,
        flags = flags,
        support = support,
        policy_name = policy_name,
        allowed = if delegation_authorized { "yes" } else { "no" },
        children = children,
        depth = depth,
        workspace = workspace,
    )
}

pub(crate) fn render_delegation_guidance(
    role: ContractRole,
    policy: &DelegationPolicy,
    delegation_authorized: bool,
) -> String {
    if !delegation_authorized {
        return "## Native Delegation\n\nWork directly. Do not create native children under the current capability/policy contract."
            .to_string();
    }

    let posture = match policy.mode {
        NativeDelegationMode::Encouraged => {
            "Proactively delegate when two or more independent, bounded lanes would materially improve speed or confidence."
        }
        NativeDelegationMode::Auto => {
            "Delegate when independent, bounded lanes would materially improve speed or confidence; otherwise work directly."
        }
        NativeDelegationMode::Disabled => unreachable!("disabled policy cannot enable delegation"),
    };
    let authority = match role {
        ContractRole::MasterPlanner | ContractRole::Queen => {
            "Native children are read-only planning, scouting, or review lanes. Visible coding principals own implementation."
        }
        ContractRole::Principal => {
            "Native children may implement only within your assigned ownership. You remain responsible for their integration and validation."
        }
        ContractRole::Researcher => {
            "Native children remain read-only and return findings to you; they do not write project files."
        }
    };

    format!(
        "## Native Delegation\n\n{posture}\n\n{authority}\n\nFor every child, state its objective, authoritative inputs, allowed paths, read/write mode, required evidence, and stop conditions. Give writing children non-overlapping ownership. Serialize shared files, migrations, lockfiles, generated artifacts, and git operations. Wait for all children, review their work, and synthesize one result. Children must not branch, commit, push, stash, reset, widen scope, or create managed Hive Workers unless their assignment explicitly authorizes it."
    )
}

pub(crate) fn render_assignment_contract(spec: &AssignmentSpec<'_>) -> String {
    format!(
        "## Assignment Contract\n\n\
- Objective: {objective}\n\
- Access: {access}\n\
- Owned scope: {owned_scope}\n\
- Authoritative input: {authoritative_input}\n\
- Deliverables:\n{deliverables}\n\
- Validation:\n{validation}\n\
- Stop and escalate when:\n{stop_conditions}",
        objective = spec.objective,
        access = spec.access,
        owned_scope = spec.owned_scope,
        authoritative_input = spec.authoritative_input,
        deliverables = render_list(spec.deliverables),
        validation = render_list(spec.validation),
        stop_conditions = render_list(spec.stop_conditions),
    )
}

pub(crate) fn render_workspace_contract(
    role: ContractRole,
    workspace_strategy: &WorkspaceStrategy,
) -> String {
    let workspace = match workspace_strategy {
        WorkspaceStrategy::SharedCell => {
            "This worktree is shared by the Queen and visible coding principals. Use explicit, non-overlapping file ownership. Do not switch branches, stash, reset, clean, or run repository-wide rewrites."
        }
        WorkspaceStrategy::IsolatedCell => {
            "Stay inside the assigned worktree. Do not edit another managed agent's worktree or assume its uncommitted changes are visible here."
        }
        WorkspaceStrategy::None => {
            "You are operating in the current project checkout. Preserve operator changes and do not create or switch branches unless explicitly authorized."
        }
    };
    let git = match (role, workspace_strategy) {
        (ContractRole::Queen, WorkspaceStrategy::SharedCell) => {
            "You own the shared cell's git state and cross-workstream integration. Keep the backend-created session branch; serialize commit, push, and PR operations after principal work is reconciled."
        }
        (ContractRole::Queen, WorkspaceStrategy::IsolatedCell) => {
            "You own cross-cell integration, push, and PR state. Keep the backend-created Queen branch and integrate only reviewed principal commits from their backend-created cell branches."
        }
        (ContractRole::Queen, WorkspaceStrategy::None) => {
            "Preserve the operator's git state. Do not create, switch, commit, or push branches unless the operator explicitly authorizes it."
        }
        (ContractRole::Principal, WorkspaceStrategy::SharedCell) => {
            "The Queen owns the shared cell's git state. Do not commit, branch, push, stash, reset, or clean."
        }
        (ContractRole::Principal, WorkspaceStrategy::IsolatedCell) => {
            "Commit the completed assignment on this backend-created cell branch as your delivery record. Do not create or switch branches, push, stash, reset, clean, or integrate other cells. The Queen owns cross-cell integration, push, and PR state."
        }
        (ContractRole::Principal, WorkspaceStrategy::None)
        | (ContractRole::MasterPlanner, _)
        | (ContractRole::Researcher, _) => {
            "Do not commit, branch, push, stash, reset, or clean."
        }
    };

    format!("## Workspace Contract\n\n{workspace}\n\n{git}")
}

fn render_list(items: &[&str]) -> String {
    if items.is_empty() {
        return "  - None specified".to_string();
    }

    items
        .iter()
        .map(|item| format!("  - {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex_config() -> AgentConfig {
        AgentConfig {
            cli: "codex".to_string(),
            model: Some("gpt-5.6".to_string()),
            ..AgentConfig::default()
        }
    }

    #[test]
    fn capability_card_distinguishes_native_children_from_managed_workers() {
        let card = render_capability_card(
            &codex_config(),
            ContractRole::Principal,
            &CapabilityCard {
                native_delegation: CapabilitySupport::Supported,
            },
            &DelegationPolicy {
                mode: NativeDelegationMode::Encouraged,
                max_children: Some(4),
                max_depth: Some(2),
            },
            &WorkspaceStrategy::SharedCell,
            true,
        );

        assert!(card.contains("Harness: `codex`"));
        assert!(card.contains("Model: `gpt-5.6`"));
        assert!(card.contains("Flags: `[]`"));
        assert!(card.contains("adapter profile, not a runtime probe"));
        assert!(card.contains("Native delegation authorized: yes"));
        assert!(card.contains("max children 4; max depth 2"));
        assert!(card.contains("not Hive Manager Workers, Cells, queue rows"));
    }

    #[test]
    fn disabled_or_unknown_auto_policy_does_not_claim_delegation() {
        let guidance = render_delegation_guidance(
            ContractRole::Principal,
            &DelegationPolicy::default(),
            false,
        );

        assert!(guidance.contains("Work directly"));
        assert!(!guidance.contains("Proactively delegate"));
    }

    #[test]
    fn encouraged_principal_children_inherit_parent_ownership() {
        let guidance = render_delegation_guidance(
            ContractRole::Principal,
            &DelegationPolicy {
                mode: NativeDelegationMode::Encouraged,
                ..DelegationPolicy::default()
            },
            true,
        );

        assert!(guidance.contains("Proactively delegate"));
        assert!(guidance.contains("only within your assigned ownership"));
        assert!(guidance.contains("non-overlapping ownership"));
    }

    #[test]
    fn shared_workspace_reserves_git_for_queen() {
        let principal =
            render_workspace_contract(ContractRole::Principal, &WorkspaceStrategy::SharedCell);
        let queen = render_workspace_contract(ContractRole::Queen, &WorkspaceStrategy::SharedCell);

        assert!(principal.contains("shared by the Queen and visible coding principals"));
        assert!(principal.contains("Queen owns the shared cell's git state"));
        assert!(principal.contains("Do not commit"));
        assert!(queen.contains("backend-created session branch"));
    }

    #[test]
    fn isolated_principal_commits_but_queen_owns_cross_cell_integration() {
        let principal =
            render_workspace_contract(ContractRole::Principal, &WorkspaceStrategy::IsolatedCell);
        let queen =
            render_workspace_contract(ContractRole::Queen, &WorkspaceStrategy::IsolatedCell);

        assert!(principal.contains("Commit the completed assignment"));
        assert!(principal.contains("Do not create or switch branches"));
        assert!(principal.contains("Queen owns cross-cell integration"));
        assert!(queen.contains("integrate only reviewed principal commits"));
    }
}
