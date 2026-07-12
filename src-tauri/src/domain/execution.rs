use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::WorkspaceStrategy;

/// Caller intent for a Hive launch.
///
/// `Auto` preserves the legacy empty-worker sentinel (empty means Solo). Explicit
/// `Hive` and `Solo` launches never change kind based on the worker roster.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum HiveLaunchKind {
    #[default]
    Auto,
    Hive,
    Solo,
}

/// How strongly a launch authorizes native delegation inside a capable CLI.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum NativeDelegationMode {
    Disabled,
    #[default]
    Auto,
    Encouraged,
}

/// Delegation authorization attached to a coordinator or principal agent.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq, Eq, Hash)]
pub struct DelegationPolicy {
    #[serde(default)]
    pub mode: NativeDelegationMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_children: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u8>,
}

impl Default for DelegationPolicy {
    fn default() -> Self {
        Self {
            mode: NativeDelegationMode::Auto,
            max_children: None,
            max_depth: None,
        }
    }
}

/// Durable execution policy for a Hive launch.
///
/// The default deliberately matches legacy sessions so adding this field is
/// backwards compatible with existing `session.json` files.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq, Eq, Hash)]
pub struct HiveExecutionPolicy {
    #[serde(default)]
    pub launch_kind: HiveLaunchKind,
    #[serde(default = "legacy_workspace_strategy")]
    pub workspace_strategy: WorkspaceStrategy,
    #[serde(default)]
    pub queen_delegation: DelegationPolicy,
    #[serde(default)]
    pub principal_delegation: DelegationPolicy,
}

impl Default for HiveExecutionPolicy {
    fn default() -> Self {
        Self {
            launch_kind: HiveLaunchKind::Auto,
            workspace_strategy: legacy_workspace_strategy(),
            queen_delegation: DelegationPolicy::default(),
            principal_delegation: DelegationPolicy::default(),
        }
    }
}

fn legacy_workspace_strategy() -> WorkspaceStrategy {
    WorkspaceStrategy::IsolatedCell
}

/// Adapter-declared support for a runtime capability.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySupport {
    Supported,
    Unsupported,
    #[default]
    Unknown,
}

/// Capability facts inferred for a CLI. Policy authorization is evaluated
/// separately so `Encouraged` never rewrites an unknown fact to supported.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct CapabilityCard {
    #[serde(default)]
    pub native_delegation: CapabilitySupport,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_policy_default_is_auto_and_isolated() {
        let policy = HiveExecutionPolicy::default();
        assert_eq!(policy.launch_kind, HiveLaunchKind::Auto);
        assert_eq!(policy.workspace_strategy, WorkspaceStrategy::IsolatedCell);
        assert_eq!(policy.queen_delegation.mode, NativeDelegationMode::Auto);
        assert_eq!(policy.principal_delegation.mode, NativeDelegationMode::Auto);
    }

    #[test]
    fn missing_policy_fields_deserialize_to_legacy_defaults() {
        let policy: HiveExecutionPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(policy, HiveExecutionPolicy::default());
    }

    #[test]
    fn frozen_wire_names_are_snake_case() {
        let policy = HiveExecutionPolicy {
            launch_kind: HiveLaunchKind::Hive,
            workspace_strategy: WorkspaceStrategy::SharedCell,
            queen_delegation: DelegationPolicy {
                mode: NativeDelegationMode::Auto,
                ..DelegationPolicy::default()
            },
            principal_delegation: DelegationPolicy {
                mode: NativeDelegationMode::Encouraged,
                ..DelegationPolicy::default()
            },
        };

        let value = serde_json::to_value(policy).unwrap();
        assert_eq!(value["launch_kind"], "hive");
        assert_eq!(value["workspace_strategy"], "shared_cell");
        assert_eq!(value["queen_delegation"]["mode"], "auto");
        assert_eq!(value["principal_delegation"]["mode"], "encouraged");
    }
}
