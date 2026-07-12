import { defaultRoles, getDefaultModel, normalizeModelId } from '$lib/config/clis';
import type {
  AgentConfig,
  DelegationMode,
  HiveLaunchConfig,
  QaWorkerConfig,
  Session,
} from '$lib/stores/sessions';

export type CodingPrincipalFormConfig = AgentConfig & {
  selectedRole: string;
  promptTemplateOverride?: string | null;
};

export interface DefaultHiveFormState {
  queenConfig: AgentConfig;
  codingPrincipals: CodingPrincipalFormConfig[];
  workspaceStrategy: HiveLaunchConfig['execution_policy']['workspace_strategy'];
  queenDelegationMode: DelegationMode;
  principalDelegationMode: DelegationMode;
  queenMaxChildren: number;
  queenMaxDepth: number;
  principalMaxChildren: number;
  principalMaxDepth: number;
}

export function createDefaultCodingPrincipal(index: number): CodingPrincipalFormConfig {
  return {
    cli: defaultRoles.principal.cli,
    model: defaultRoles.principal.model,
    flags: [],
    label: `Coding Principal ${index + 1}`,
    selectedRole: 'principal',
    promptTemplateOverride: null,
  };
}

export function nextCodingPrincipalIndex(
  principals: Array<Pick<AgentConfig, 'label'>>,
): number {
  const usedLabels = new Set(principals.map((principal) => principal.label));
  let index = 0;
  while (usedLabels.has(`Coding Principal ${index + 1}`)) index += 1;
  return index;
}

export function automaticAdversarialLaneCount(
  principalCount: number,
  qaWorkers: Array<Pick<QaWorkerConfig, 'specialization'>>,
): number {
  const target = Math.ceil(Math.max(0, principalCount) / 2);
  const configured = qaWorkers.filter(
    (worker) => worker.specialization.toLowerCase() === 'adversarial',
  ).length;
  return Math.max(0, target - configured);
}

/**
 * Resolve the config for a principal added to an existing session.
 *
 * This mirrors the backend's legacy fallback contract: modern sessions use
 * their durable principal defaults, while sessions created before those fields
 * existed inherit the historical session/Queen CLI and model.
 */
export function createSessionPrincipalConfig(
  session: Pick<
    Session,
    | 'default_cli'
    | 'default_model'
    | 'default_principal_cli'
    | 'default_principal_model'
    | 'default_principal_flags'
  > | null,
): AgentConfig {
  const explicitPrincipalCli = session?.default_principal_cli?.trim() || undefined;
  const cli = explicitPrincipalCli
    ?? session?.default_cli?.trim()
    ?? defaultRoles.principal.cli;
  const configuredModel = explicitPrincipalCli
    ? session?.default_principal_model
    : session?.default_model;
  const configuredModelId = configuredModel?.trim() || getDefaultModel(cli) || undefined;
  const model = configuredModelId
    ? normalizeModelId(cli, configuredModelId)
    : undefined;

  return {
    cli,
    model,
    flags: explicitPrincipalCli
      ? [...(session?.default_principal_flags ?? [])]
      : [],
  };
}

export function createDefaultHiveFormState(): DefaultHiveFormState {
  return {
    queenConfig: {
      cli: defaultRoles.queen.cli,
      model: defaultRoles.queen.model,
      flags: [],
      label: 'Queen',
    },
    codingPrincipals: [createDefaultCodingPrincipal(0)],
    workspaceStrategy: 'shared_cell',
    queenDelegationMode: 'auto',
    principalDelegationMode: 'encouraged',
    queenMaxChildren: 3,
    queenMaxDepth: 1,
    principalMaxChildren: 2,
    principalMaxDepth: 1,
  };
}

function buildDelegationPolicy(
  mode: DelegationMode,
  maxChildren: number,
  maxDepth: number,
): HiveLaunchConfig['execution_policy']['queen_delegation'] {
  if (mode === 'disabled') return { mode };
  return {
    mode,
    max_children: Math.max(1, maxChildren),
    max_depth: Math.max(1, maxDepth),
  };
}

export interface BuildHiveLaunchConfigInput {
  name?: string;
  color?: string;
  projectPath: string;
  queenConfig: AgentConfig;
  principals: AgentConfig[];
  workspaceStrategy: HiveLaunchConfig['execution_policy']['workspace_strategy'];
  queenDelegationMode: DelegationMode;
  principalDelegationMode: DelegationMode;
  queenMaxChildren: number;
  queenMaxDepth: number;
  principalMaxChildren: number;
  principalMaxDepth: number;
  prompt?: string;
  withPlanning: boolean;
  smokeTest: boolean;
  withEvaluator: boolean;
  evaluatorConfig?: AgentConfig;
  qaWorkers?: QaWorkerConfig[];
}

export function buildHiveLaunchConfig(input: BuildHiveLaunchConfigInput): HiveLaunchConfig {
  return {
    name: input.name,
    color: input.color,
    project_path: input.projectPath,
    queen_config: input.queenConfig,
    workers: input.principals,
    execution_policy: {
      launch_kind: 'hive',
      workspace_strategy: input.workspaceStrategy,
      queen_delegation: buildDelegationPolicy(
        input.queenDelegationMode,
        input.queenMaxChildren,
        input.queenMaxDepth,
      ),
      principal_delegation: buildDelegationPolicy(
        input.principalDelegationMode,
        input.principalMaxChildren,
        input.principalMaxDepth,
      ),
    },
    prompt: input.prompt,
    with_planning: input.withPlanning,
    smoke_test: input.smokeTest,
    with_evaluator: input.withEvaluator,
    evaluator_config: input.withEvaluator ? input.evaluatorConfig : undefined,
    qa_workers: input.withEvaluator ? input.qaWorkers : undefined,
  };
}
