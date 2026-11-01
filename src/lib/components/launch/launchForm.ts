import type { TaskTier } from '$lib/workgraph/types';

export type SessionMode = 'templates' | 'hive' | 'fusion' | 'solo' | 'research' | 'debate';
export type WorkGraphParameterRow = { id: number; key: string; value: string };
export type TierLadderPreviewCell = {
  provider: string;
  tier: TaskTier;
  model: string;
  flags: string[];
};
export type TierLadderOmission = {
  reason: string;
  sourceRef: string;
  detail: string;
};
export type TierLadderPreviewResponse = {
  cells: TierLadderPreviewCell[];
  omissions: TierLadderOmission[];
};

export const TASK_TIERS: TaskTier[] = ['low', 'medium', 'high', 'critical'];

export const WORK_GRAPH_ARCHETYPES = [
  { id: 'feature-build', label: 'Feature build' },
  { id: 'bug-hunt', label: 'Bug hunt' },
  { id: 'migration', label: 'Migration' },
  { id: 'audit', label: 'Audit' },
];

export function isTaskTier(value: unknown): value is TaskTier {
  return typeof value === 'string' && TASK_TIERS.includes(value as TaskTier);
}

export function normalizeTierLadderPreview(payload: unknown): TierLadderPreviewResponse {
  if (typeof payload !== 'object' || payload === null) {
    throw new Error('Tier ladder response was invalid');
  }
  const candidate = payload as { cells?: unknown; omissions?: unknown };
  if (!Array.isArray(candidate.cells) || !Array.isArray(candidate.omissions)) {
    throw new Error('Tier ladder response was invalid');
  }

  const cells = candidate.cells.filter((entry): entry is TierLadderPreviewCell => {
    if (typeof entry !== 'object' || entry === null) return false;
    const cell = entry as Record<string, unknown>;
    return typeof cell.provider === 'string'
      && isTaskTier(cell.tier)
      && typeof cell.model === 'string'
      && Array.isArray(cell.flags)
      && cell.flags.every((flag) => typeof flag === 'string');
  });
  const omissions = candidate.omissions.filter((entry): entry is TierLadderOmission => {
    if (typeof entry !== 'object' || entry === null) return false;
    const omission = entry as Record<string, unknown>;
    return typeof omission.reason === 'string'
      && typeof omission.sourceRef === 'string'
      && typeof omission.detail === 'string';
  });

  return { cells, omissions };
}

// ... (predefinedRoles same)
// CLI defaults match backend default_roles in storage/mod.rs
export const predefinedRoles = [
  {
    type: 'principal',
    label: 'Coding Principal',
    cli: 'codex',
    description: 'Owns an implementation lane and delegates bounded subtasks',
    promptTemplate: null,
  },
  {
    type: 'backend',
    label: 'Backend',
    cli: 'claude',
    description: 'Backend code, APIs, databases',
    promptTemplate: `You are the BACKEND specialist. Focus on:
- Server-side logic, APIs, and database operations
- Authentication, authorization, and security
- Performance optimization and caching
- Error handling and logging
- Data validation and sanitization
Do NOT work on frontend/UI code unless it directly interfaces with your backend work.`
  },
  {
    type: 'frontend',
    label: 'Frontend',
    cli: 'codex',
    description: 'UI components, styling, UX',
    promptTemplate: `You are the FRONTEND specialist. Focus on:
- UI components, layouts, and styling
- User interactions and state management
- Accessibility and responsive design
- Client-side validation and error display
- Performance optimization (lazy loading, code splitting)
Do NOT work on backend/server code unless it directly interfaces with your frontend work.`
  },
  {
    type: 'coherence',
    label: 'Coherence',
    cli: 'codex',
    description: 'Ensures code consistency',
    promptTemplate: `You are the COHERENCE specialist. Focus on:
- Ensuring consistent code patterns across the codebase
- Verifying naming conventions are followed
- Checking that similar problems are solved similarly
- Identifying and unifying duplicate logic
- Ensuring documentation matches implementation
Review changes made by other workers and flag inconsistencies.`
  },
  {
    type: 'simplify',
    label: 'Simplify',
    cli: 'codex',
    description: 'Refactors and simplifies code',
    promptTemplate: `You are the SIMPLIFY specialist. Focus on:
- Reducing code complexity and nesting
- Extracting reusable functions and components
- Removing dead code and unused imports
- Improving readability and maintainability
- Simplifying conditional logic
Review changes made by other workers and suggest simplifications.`
  },
  {
    type: 'reviewer',
    label: 'Reviewer',
    cli: 'claude',
    description: 'Reviews code for issues',
    promptTemplate: `You are the REVIEWER specialist. Focus on:
- Identifying bugs, edge cases, and potential issues
- Checking for security vulnerabilities
- Verifying error handling is comprehensive
- Ensuring tests cover critical paths
- Validating changes match requirements
Create detailed review comments for issues found. Do NOT fix issues yourself - that's the Resolver's job.`
  },
  {
    type: 'resolver',
    label: 'Resolver',
    cli: 'claude',
    description: 'Resolves reviewer issues',
    promptTemplate: `You are the RESOLVER specialist. Focus on:
- Addressing issues identified by the Reviewer
- Implementing fixes for bugs and edge cases
- Adding missing error handling
- Writing tests for uncovered paths
- Responding to review comments with fixes
Wait for Reviewer feedback before making changes. Your job is to resolve their concerns.`
  },
  {
    type: 'code-quality',
    label: 'Code Quality',
    cli: 'claude',
    description: 'PR comments, linting, tests',
    promptTemplate: `You are the CODE QUALITY specialist. Focus on:
- Running and fixing linter errors
- Ensuring test coverage meets standards
- Resolving PR review comments
- Fixing type errors and warnings
- Ensuring CI/CD checks pass
Use /resolveprcomments style workflow to systematically address quality issues.`
  },
  {
    type: 'general',
    label: 'General',
    cli: 'claude',
    description: 'General purpose worker',
    promptTemplate: null
  },
];

export const COLORS = [
  { name: 'Blue', value: '#7aa2f7' },
  { name: 'Purple', value: '#bb9af7' },
  { name: 'Green', value: '#9ece6a' },
  { name: 'Yellow', value: '#e0af68' },
  { name: 'Cyan', value: '#7dcfff' },
  { name: 'Red', value: '#f7768e' },
  { name: 'Orange', value: '#ff9e64' },
  { name: 'Pink', value: '#f7b1d1' },
];

export function providerLabel(provider: string): string {
  return provider.charAt(0).toUpperCase() + provider.slice(1);
}
