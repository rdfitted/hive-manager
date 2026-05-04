/**
 * Shared CLI options configuration
 * Single source of truth for frontend CLI lists
 * Model defaults must match backend cli/registry.rs and storage/mod.rs
 */

export interface CliOption {
  value: string;
  label: string;
  description: string;
  /** Default model for this CLI (from backend registry.rs::default_model) */
  defaultModel: string;
}

/**
 * All available CLI options for agent configuration
 */
export const cliOptions: CliOption[] = [
  { value: 'claude', label: 'Claude Code', description: 'Anthropic Claude', defaultModel: 'opus' },
  { value: 'gemini', label: 'Gemini CLI', description: 'Google Gemini Pro', defaultModel: 'gemini-2.5-pro' },
  { value: 'opencode', label: 'OpenCode', description: 'BigPickle, Grok, multi-model', defaultModel: 'opencode/big-pickle' },
  { value: 'codex', label: 'Codex', description: 'OpenAI GPT-5.5', defaultModel: 'gpt-5.5' },
  { value: 'cursor', label: 'Cursor', description: 'Cursor CLI via WSL (Composer 2)', defaultModel: 'composer-2' },
  { value: 'droid', label: 'Droid', description: 'GLM 5.1 (Factory Droid CLI)', defaultModel: 'glm-5.1' },
  { value: 'qwen', label: 'Qwen', description: 'Qwen Code CLI (Qwen3-Coder)', defaultModel: 'qwen3-coder' },
];

/**
 * Default CLI and model assignments per role type
 * Must match backend default_roles in storage/mod.rs
 */
export interface RoleDefaults {
  cli: string;
  model: string;
}

export const defaultRoles: Record<string, RoleDefaults> = {
  backend: { cli: 'codex', model: 'gpt-5.5' },
  frontend: { cli: 'gemini', model: 'gemini-2.5-pro' },
  coherence: { cli: 'codex', model: 'gpt-5.5' },
  simplify: { cli: 'codex', model: 'gpt-5.5' },
  // Review & QA roles
  reviewer: { cli: 'codex', model: 'gpt-5.5' },
  'reviewer-quick': { cli: 'codex', model: 'gpt-5.5' },
  resolver: { cli: 'codex', model: 'gpt-5.5' },
  tester: { cli: 'codex', model: 'gpt-5.5' },
  'code-quality': { cli: 'codex', model: 'gpt-5.5' },
  // Evaluator & QA roles - match backend storage/mod.rs default_roles
  evaluator: { cli: 'claude', model: 'opus' },
  'qa-worker': { cli: 'codex', model: 'gpt-5.5' },
  // General purpose
  general: { cli: 'codex', model: 'gpt-5.5' },
};

/**
 * Get the default model for a CLI
 */
export function getDefaultModel(cli: string): string {
  return cliOptions.find(c => c.value === cli)?.defaultModel ?? 'opus';
}
