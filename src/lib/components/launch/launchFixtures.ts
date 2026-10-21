import type { SessionTemplate } from '$lib/types/domain';

// Independent fixtures captured from builtin_session_templates at 14ffe2d.
// Do not derive the oracle from production form defaults or payload builders.
const cell = (role: string, cli: string, model: string, prompt_template: string) => ({ role, cli, model, prompt_template });
export const builtinFixtures: SessionTemplate[] = [
  {
    id: 'bug-fix-hive', name: 'Bug-fix Hive',
    description: 'Opus Queen with GPT-6 backend and frontend coding principals.',
    mode: 'hive', workspace_strategy: 'shared_cell', is_builtin: true,
    work_graph_archetype: null, work_graph_parameters: {},
    cells: [cell('queen', 'claude', 'opus', 'queen-hive'), cell('backend', 'codex', 'gpt-6.1-sol', 'roles/backend'), cell('frontend', 'codex', 'gpt-6.1-sol', 'roles/frontend')],
  },
  {
    id: 'feature-build-hive', name: 'Feature-build Hive',
    description: 'Opus Queen plus GPT-6 backend/frontend coding principals and a Droid coherence specialist.',
    mode: 'hive', workspace_strategy: 'shared_cell', is_builtin: true,
    work_graph_archetype: null, work_graph_parameters: {},
    cells: [cell('queen', 'claude', 'opus', 'queen-hive'), cell('backend', 'codex', 'gpt-6.1-sol', 'roles/backend'), cell('frontend', 'codex', 'gpt-6.1-sol', 'roles/frontend'), cell('coherence', 'droid', 'glm-5.1', 'roles/coherence')],
  },
  {
    id: 'fusion-compare', name: 'Fusion Compare',
    description: 'Two candidate implementation cells plus a resolver recommendation pass.',
    mode: 'fusion', workspace_strategy: 'isolated_cell', is_builtin: true,
    work_graph_archetype: null, work_graph_parameters: {},
    cells: [cell('candidate-a', 'codex', 'gpt-6.1-sol', 'fusion-worker'), cell('candidate-b', 'codex', 'gpt-6-luna', 'fusion-worker'), cell('resolver', 'claude', 'opus', 'resolver')],
  },
];

export const legacyQueen = { cli: 'claude', model: 'opus', flags: [], label: 'Queen' };
export const legacyEvaluator = { cli: 'claude', model: 'opus', flags: [], label: 'Evaluator' };
export const legacyQa = ['ui', 'api', 'a11y'].map((specialization) => ({ specialization, cli: 'codex', model: 'gpt-6.1-sol', flags: [] }));
export const legacyPolicy = {
  launch_kind: 'hive', workspace_strategy: 'shared_cell',
  queen_delegation: { mode: 'auto', max_children: 3, max_depth: 1 },
  principal_delegation: { mode: 'encouraged', max_children: 2, max_depth: 1 },
  tier_policy: { enabled: false, ceiling_percent: 34, review_floor: 'high', ladder: {} },
};

export function legacyPayload(mode: string, project: string, task: string, template?: SessionTemplate, count?: number): Record<string, unknown> {
  const common = { name: template?.name, color: undefined, project_path: project };
  if (mode === 'hive') {
    const workers = template
      ? template.cells.filter((entry) => entry.role !== 'queen').map((entry) => ({
        cli: entry.cli, model: entry.model, flags: [],
        label: entry.role.charAt(0).toUpperCase() + entry.role.slice(1),
        role: { role_type: entry.role, label: entry.role.charAt(0).toUpperCase() + entry.role.slice(1), default_cli: entry.cli, prompt_template: entry.prompt_template },
      }))
      : Array.from({ length: count ?? 1 }, (_, index) => ({
        cli: 'codex', model: 'gpt-6.1-sol', flags: [], label: `Coding Principal ${index + 1}`,
        role: { role_type: 'principal', label: 'Coding Principal', default_cli: 'codex', prompt_template: null },
      }));
    return { ...common, queen_config: legacyQueen, workers, execution_policy: legacyPolicy, work_graph_archetype: null, work_graph_parameters: {}, prompt: task || undefined, with_planning: true, smoke_test: false, with_evaluator: true, evaluator_config: legacyEvaluator, qa_workers: legacyQa };
  }
  if (mode === 'research') return {
    ...common, queen_config: legacyQueen,
    workers: Array.from({ length: count ?? 2 }, (_, index) => ({ cli: 'codex', model: 'gpt-6.1-sol', flags: [], label: `Researcher ${index + 1}`, role: { role_type: 'researcher', label: 'Researcher', default_cli: 'codex', prompt_template: null } })),
    prompt: task || undefined, with_planning: false, with_evaluator: false, smoke_test: false,
  };
  if (mode === 'solo') return { name: undefined, color: undefined, projectPath: project, taskDescription: task.trim() || undefined, cli: 'claude', model: undefined, flags: [], with_evaluator: true, evaluator_config: legacyEvaluator, qa_workers: legacyQa };
  if (mode === 'fusion') return {
    ...common,
    variants: template
      ? [ { name: 'Variant A', cli: 'codex', model: 'gpt-6.1-sol', flags: [] }, { name: 'Variant B', cli: 'codex', model: 'gpt-6-luna', flags: [] } ]
      : Array.from({ length: count ?? 2 }, (_, index) => ({ name: `Variant ${String.fromCharCode(65 + index)}`, cli: 'claude', flags: [] })),
    task_description: task, judge_config: template ? { cli: 'claude', model: 'opus', flags: [], label: 'Fusion Resolver' } : { cli: 'claude' }, queen_config: legacyQueen, with_planning: true,
  };
  if (mode === 'debate') return { ...common, debaters: Array.from({ length: count ?? 2 }, (_, index) => ({ name: `Debater ${String.fromCharCode(65 + index)}`, stance: ['Proponent', 'Opponent', 'Alternative A', 'Alternative B'][index], cli: 'claude', flags: [] })), topic: task, rounds: 3, judge_config: { cli: 'claude', flags: [], label: 'Debate Judge' }, queen_config: legacyQueen, with_planning: true, default_cli: 'claude', default_model: undefined };
  throw new Error('Templates has no launch payload');
}
