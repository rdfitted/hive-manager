/**
 * Shared CLI options configuration
 * Single source of truth for frontend CLI lists
 * Model defaults must match backend cli/registry.rs and storage/mod.rs
 */

export interface CliOption {
  value: string;
  label: string;
  description: string;
  /**
   * Default model for this CLI (from backend registry.rs::default_model).
   * Empty string means the CLI has no model flag and the UI should hide the
   * model field (currently: antigravity, whose model lives in
   * `~/.gemini/antigravity-cli/settings.json`).
   */
  defaultModel: string;
}

/**
 * All available CLI options for agent configuration
 */
export const cliOptions: CliOption[] = [
  { value: 'claude', label: 'Claude Code', description: 'Anthropic Claude', defaultModel: 'opus' },
  { value: 'gemini', label: 'Gemini CLI', description: 'Google Gemini Pro (deprecates 2026-06-18 — use Antigravity for new work)', defaultModel: 'gemini-2.5-pro' },
  { value: 'antigravity', label: 'Antigravity CLI', description: 'Google Antigravity (agy) mixed-model option. Model + verbosity are set globally in ~/.gemini/antigravity-cli/settings.json.', defaultModel: '' },
  { value: 'opencode', label: 'OpenCode', description: 'BigPickle, Grok, multi-model', defaultModel: 'opencode/big-pickle' },
  { value: 'codex', label: 'Codex', description: 'OpenAI GPT-5.6 / Sol', defaultModel: 'gpt-5.6' },
  { value: 'cursor', label: 'Cursor', description: 'Cursor CLI via WSL (Composer 2.5)', defaultModel: 'composer-2.5' },
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
  queen: { cli: 'claude', model: 'opus' },
  principal: { cli: 'codex', model: 'gpt-5.6' },
  backend: { cli: 'codex', model: 'gpt-5.6' },
  // Coding roles intentionally share the Codex default.
  frontend: { cli: 'codex', model: 'gpt-5.6' },
  coherence: { cli: 'codex', model: 'gpt-5.6' },
  simplify: { cli: 'codex', model: 'gpt-5.6' },
  // Review & QA roles
  reviewer: { cli: 'codex', model: 'gpt-5.6' },
  'reviewer-quick': { cli: 'codex', model: 'gpt-5.6' },
  resolver: { cli: 'codex', model: 'gpt-5.6' },
  tester: { cli: 'codex', model: 'gpt-5.6' },
  'code-quality': { cli: 'codex', model: 'gpt-5.6' },
  // Evaluator & QA roles - match backend storage/mod.rs default_roles
  evaluator: { cli: 'claude', model: 'opus' },
  'qa-worker': { cli: 'codex', model: 'gpt-5.6' },
  // General purpose
  general: { cli: 'codex', model: 'gpt-5.6' },
};

/**
 * Get the default model for a CLI
 */
export function getDefaultModel(cli: string): string {
  return cliOptions.find(c => c.value === cli)?.defaultModel ?? 'opus';
}
