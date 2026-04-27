/**
 * Shared CLI options configuration
 * Single source of truth for frontend CLI lists
 */

export interface CliOption {
  value: string;
  label: string;
  description: string;
}

/**
 * All available CLI options for agent configuration
 */
export const cliOptions: CliOption[] = [
  { value: 'claude', label: 'Claude Code', description: 'Anthropic Claude (Opus 4.7)' },
  { value: 'gemini', label: 'Gemini CLI', description: 'Google Gemini Pro' },
  { value: 'opencode', label: 'OpenCode', description: 'BigPickle, Grok, multi-model' },
  { value: 'codex', label: 'Codex', description: 'OpenAI GPT-5.5' },
  { value: 'cursor', label: 'Cursor', description: 'Cursor CLI via WSL (Opus 4.7)' },
  { value: 'droid', label: 'Droid', description: 'GLM 5.1 (Factory Droid CLI)' },
  { value: 'qwen', label: 'Qwen', description: 'Qwen Code CLI (Qwen3-Coder)' },
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
  backend: { cli: 'claude', model: 'opus-4-7' },
  frontend: { cli: 'gemini', model: 'gemini-2.5-pro' },
  coherence: { cli: 'droid', model: 'glm-5.1' },
  simplify: { cli: 'codex', model: 'gpt-5.5' },
  // Review & QA roles default to claude
  reviewer: { cli: 'claude', model: 'claude-opus-4-7' },
  'reviewer-quick': { cli: 'claude', model: 'claude-opus-4-7' },
  resolver: { cli: 'claude', model: 'claude-opus-4-7' },
  tester: { cli: 'claude', model: 'claude-opus-4-7' },
  'code-quality': { cli: 'codex', model: 'gpt-5.5' },
  // Evaluator & QA roles
  evaluator: { cli: 'claude', model: 'claude-opus-4-7' },
  'qa-worker': { cli: 'claude', model: 'claude-opus-4-7' },
  // General purpose
  general: { cli: 'claude', model: 'claude-opus-4-7' },
};
