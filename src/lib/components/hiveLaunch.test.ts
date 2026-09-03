import { describe, expect, it } from 'vitest';
import {
  automaticAdversarialLaneCount,
  buildHiveLaunchConfig,
  createDefaultHiveFormState,
  createSessionPrincipalConfig,
  nextCodingPrincipalIndex,
} from './hiveLaunch';

describe('default Hive launch contract', () => {
  it('uses Opus Queen, GPT-5.6 principal, shared workspace, and split delegation defaults', () => {
    const defaults = createDefaultHiveFormState();
    const config = buildHiveLaunchConfig({
      projectPath: 'C:/code/project',
      queenConfig: defaults.queenConfig,
      principals: defaults.codingPrincipals,
      workspaceStrategy: defaults.workspaceStrategy,
      queenDelegationMode: defaults.queenDelegationMode,
      principalDelegationMode: defaults.principalDelegationMode,
      queenMaxChildren: defaults.queenMaxChildren,
      queenMaxDepth: defaults.queenMaxDepth,
      principalMaxChildren: defaults.principalMaxChildren,
      principalMaxDepth: defaults.principalMaxDepth,
      workGraphArchetype: defaults.workGraphArchetype,
      workGraphParameters: defaults.workGraphParameters,
      withPlanning: true,
      smokeTest: false,
      withEvaluator: true,
    });

    expect(config.queen_config).toMatchObject({ cli: 'claude', model: 'opus' });
    expect(config.workers).toHaveLength(1);
    expect(config.workers[0]).toMatchObject({
      cli: 'codex',
      model: 'gpt-5.6-sol',
      label: 'Coding Principal 1',
    });
    expect(config.execution_policy).toEqual({
      launch_kind: 'hive',
      workspace_strategy: 'shared_cell',
      queen_delegation: { mode: 'auto', max_children: 3, max_depth: 1 },
      principal_delegation: { mode: 'encouraged', max_children: 2, max_depth: 1 },
      tier_policy: {
        enabled: false,
        ceiling_percent: 34,
        review_floor: 'high',
        ladder: {},
      },
    });
    expect(config.work_graph_archetype).toBeNull();
    expect(config.work_graph_parameters).toEqual({});
  });

  it('transports an enabled tier-routing policy without a client-owned ladder snapshot', () => {
    const defaults = createDefaultHiveFormState();
    const config = buildHiveLaunchConfig({
      projectPath: 'C:/code/project',
      queenConfig: defaults.queenConfig,
      principals: defaults.codingPrincipals,
      workspaceStrategy: defaults.workspaceStrategy,
      queenDelegationMode: defaults.queenDelegationMode,
      principalDelegationMode: defaults.principalDelegationMode,
      queenMaxChildren: defaults.queenMaxChildren,
      queenMaxDepth: defaults.queenMaxDepth,
      principalMaxChildren: defaults.principalMaxChildren,
      principalMaxDepth: defaults.principalMaxDepth,
      tierPolicy: {
        enabled: true,
        ceiling_percent: 72,
        review_floor: 'critical',
        ladder: { stale: 'preview-only' },
      },
      workGraphArchetype: null,
      workGraphParameters: {},
      withPlanning: true,
      smokeTest: false,
      withEvaluator: false,
    });

    expect(config.execution_policy.tier_policy).toEqual({
      enabled: true,
      ceiling_percent: 72,
      review_floor: 'critical',
      ladder: {},
    });
  });

  it('transports a selected work-graph archetype and normalized overrides', () => {
    const defaults = createDefaultHiveFormState();
    const config = buildHiveLaunchConfig({
      projectPath: 'C:/code/project',
      queenConfig: defaults.queenConfig,
      principals: defaults.codingPrincipals,
      workspaceStrategy: defaults.workspaceStrategy,
      queenDelegationMode: defaults.queenDelegationMode,
      principalDelegationMode: defaults.principalDelegationMode,
      queenMaxChildren: defaults.queenMaxChildren,
      queenMaxDepth: defaults.queenMaxDepth,
      principalMaxChildren: defaults.principalMaxChildren,
      principalMaxDepth: defaults.principalMaxDepth,
      workGraphArchetype: ' feature-build ',
      workGraphParameters: {
        ' component ': ' launch surface ',
        empty: ' ',
        ' ': 'ignored',
      },
      withPlanning: false,
      smokeTest: false,
      withEvaluator: false,
    });

    expect(config.work_graph_archetype).toBe('feature-build');
    expect(config.work_graph_parameters).toEqual({ component: 'launch surface' });
    expect(config.with_planning).toBe(true);
  });

  it('drops stale overrides when no archetype is selected', () => {
    const defaults = createDefaultHiveFormState();
    const config = buildHiveLaunchConfig({
      projectPath: 'C:/code/project',
      queenConfig: defaults.queenConfig,
      principals: defaults.codingPrincipals,
      workspaceStrategy: defaults.workspaceStrategy,
      queenDelegationMode: defaults.queenDelegationMode,
      principalDelegationMode: defaults.principalDelegationMode,
      queenMaxChildren: defaults.queenMaxChildren,
      queenMaxDepth: defaults.queenMaxDepth,
      principalMaxChildren: defaults.principalMaxChildren,
      principalMaxDepth: defaults.principalMaxDepth,
      workGraphArchetype: null,
      workGraphParameters: { component: 'must-not-leak' },
      withPlanning: false,
      smokeTest: false,
      withEvaluator: false,
    });

    expect(config.work_graph_archetype).toBeNull();
    expect(config.work_graph_parameters).toEqual({});
    expect(config.with_planning).toBe(false);
  });

  it('normalizes legacy Sol defaults for workers added after launch', () => {
    expect(createSessionPrincipalConfig({
      default_cli: 'claude',
      default_model: 'opus',
      default_principal_cli: 'codex',
      default_principal_model: 'gpt-5.6',
      default_principal_flags: ['-c', 'model_reasoning_effort="xhigh"'],
    })).toEqual({
      cli: 'codex',
      model: 'gpt-5.6-sol',
      flags: ['-c', 'model_reasoning_effort="xhigh"'],
    });
  });

  it('preserves the legacy session defaults when principal fields are absent', () => {
    expect(createSessionPrincipalConfig({
      default_cli: 'claude',
      default_model: 'fable',
    })).toEqual({
      cli: 'claude',
      model: 'fable',
      flags: [],
    });
  });

  it('chooses an unused principal label after a middle principal is removed', () => {
    expect(nextCodingPrincipalIndex([
      { label: 'Coding Principal 1' },
      { label: 'Coding Principal 3' },
    ])).toBe(1);
  });

  it('fills only the missing adversarial lanes to the one-per-two target', () => {
    expect(automaticAdversarialLaneCount(3, [
      { specialization: 'ui' },
      { specialization: 'adversarial' },
    ])).toBe(1);
    expect(automaticAdversarialLaneCount(3, [
      { specialization: 'adversarial' },
      { specialization: 'adversarial' },
    ])).toBe(0);
  });
});
