import { writable, derived } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { CellStatus } from '$lib/types/domain';
import { applicationState } from './applicationState';
import { ui } from './ui';

export type AgentRole =
  | 'MasterPlanner'
  | 'Queen'
  | 'Evaluator'
  | { Judge: { session_id: string } }
  | { Planner: { index: number } }
  | { Worker: { index: number; parent: string | null } }
  | { QaWorker: { index: number; parent: string | null } }
  | { Fusion: { variant: string } };

export type AgentStatus = 
  | 'Starting' 
  | 'Running' 
  | { WaitingForInput: string } 
  | 'Completed' 
  | { Error: string };

export interface WorkerRole {
  role_type: string;
  label: string;
  default_cli: string;
  prompt_template: string | null;
}

export interface AgentConfig {
  cli: string;
  model?: string;
  flags: string[];
  label?: string;
  name?: string;
  description?: string;
  role?: WorkerRole;
  initial_prompt?: string;
}

export interface AgentInfo {
  id: string;
  role: AgentRole;
  status: AgentStatus;
  config: AgentConfig;
  parent_id: string | null;
}

export interface HiveLaunchConfig {
  name?: string;
  color?: string;
  project_path: string;
  queen_config: AgentConfig;
  workers: AgentConfig[];
  prompt?: string;
  with_planning?: boolean;
  with_evaluator?: boolean;
  evaluator_config?: AgentConfig;
  qa_workers?: QaWorkerConfig[];
  smoke_test?: boolean;
}

export interface ResearchLaunchConfig {
  name?: string;
  color?: string;
  project_path: string;
  queen_config: AgentConfig;
  workers: AgentConfig[];
  prompt?: string;
  with_planning?: boolean;
  with_evaluator?: boolean;
  evaluator_config?: AgentConfig;
  qa_workers?: QaWorkerConfig[];
  smoke_test?: boolean;
}

export interface QaWorkerConfig {
  specialization: 'ui' | 'api' | 'a11y';
  cli: string;
  model?: string;
  flags: string[];
  label?: string;
}

export interface FusionVariantConfig {
  name: string;
  cli: string;
  model?: string;
  flags?: string[];
}

export interface FusionLaunchConfig {
  name?: string;
  color?: string;
  project_path: string;
  variants: FusionVariantConfig[];
  task_description: string;
  judge_config: { cli: string; model?: string; flags?: string[]; label?: string };
  queen_config?: { cli: string; model?: string; flags?: string[]; label?: string };
  with_planning: boolean;
}

export interface DebateDebaterConfig {
  name: string;
  stance?: string;
  cli: string;
  model?: string;
  flags: string[];
}

export interface DebateLaunchConfig {
  project_path: string;
  name?: string;
  color?: string;
  debaters: DebateDebaterConfig[];
  topic: string;
  rounds: number;
  judge_config: AgentConfig;
  queen_config?: AgentConfig;
  with_planning: boolean;
  default_cli: string;
  default_model?: string;
}

export interface PlannerConfig {
  config: AgentConfig;
  domain: string;
  workers: AgentConfig[];
}

export interface SwarmLaunchConfig {
  name?: string;
  color?: string;
  project_path: string;
  queen_config: AgentConfig;
  planner_count: number;                  // How many planners
  planner_config: AgentConfig;            // Config shared by all planners
  workers_per_planner: AgentConfig[];     // Workers config (applied to each planner)
  prompt?: string;
  with_planning?: boolean;
  with_evaluator?: boolean;
  evaluator_config?: AgentConfig;
  qa_workers?: QaWorkerConfig[];
  smoke_test?: boolean;
}

export interface SoloLaunchConfig {
  name?: string;
  color?: string;
  projectPath: string;
  taskDescription?: string;
  cli: string;
  model?: string;
  with_evaluator?: boolean;
  evaluator_config?: AgentConfig;
  qa_workers?: QaWorkerConfig[];
}

export type SessionState =
  | 'Planning'
  | 'PlanReady'
  | 'Starting'
  | 'Running'
  | 'SpawningEvaluator'
  | 'QaInProgress'
  | 'QaPassed'
  | { QaFailed: { iteration: number } }
  | 'QaMaxRetriesExceeded'
  | 'Paused'
  | 'Completed'
  | 'Closed'
  | { Failed: string };

/** Serde externally-tagged enums from Tauri: unit variants are often `{ Queen: null }`, not `"Queen"`. */
export function serdeEnumVariantName(value: unknown): string | undefined {
  if (typeof value === 'string') return value;
  if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    const keys = Object.keys(value as Record<string, unknown>);
    if (keys.length === 1) return keys[0];
  }
  return undefined;
}

export function sessionStateToCellStatus(state: SessionState | unknown): CellStatus {
  const key = serdeEnumVariantName(state) ?? 'Unknown';

  switch (key) {
    case 'Planning':
    case 'PlanReady':
      return 'preparing';
    case 'Starting':
    case 'SpawningWorker':
    case 'SpawningPlanner':
    case 'SpawningFusionVariant':
    case 'SpawningJudge':
    case 'SpawningEvaluator':
      return 'launching';
    case 'WaitingForWorker':
    case 'WaitingForPlanner':
    case 'WaitingForFusionVariants':
    case 'Judging':
    case 'MergingWinner':
    case 'QaInProgress':
    case 'Running':
      return 'running';
    case 'AwaitingVerdictSelection':
    case 'Paused':
    case 'QaPassed':
      return 'waiting_input';
    case 'Completed':
    case 'Closed':
      return 'completed';
    case 'QaFailed':
    case 'QaMaxRetriesExceeded':
    case 'Failed':
      return 'failed';
    case 'Closing':
      return 'summarizing';
    default:
      return 'queued';
  }
}

export interface Session {
  id: string;
  name?: string;
  color?: string;
  session_type: 
    | { Hive: { worker_count: number } } 
    | { Swarm: { planner_count: number } } 
    | { Fusion: { variants: string[] } }
    | { Debate: { variants: string[] } }
    | { Solo: { cli: string } };
  project_path: string;
  state: SessionState;
  created_at: string;
  /** RFC3339; omitted on older persisted sessions — UI falls back to `created_at`. */
  last_activity_at?: string;
  agents: AgentInfo[];
  /** Git worktree path for the session primary workspace (Tauri Session), when set. */
  worktree_path?: string | null;
  worktree_branch?: string | null;
  /** Present on a resumed session (#125): per-step classification for the resume modal. */
  resume_report?: ResumeReport | null;
}

// ---- #125 run journal + side-effect ledger ----

/** The kind of orchestrator step journaled (mirrors Rust `StepKind`, snake_case). */
export type StepKind =
  | 'worker_spawn'
  | 'evaluator_spawn'
  | 'git_commit'
  | 'git_branch'
  | 'file_write'
  | 'task_injection'
  | 'other';

/** Lifecycle status of a journaled step (mirrors Rust `StepStatus`, snake_case). */
export type StepStatus =
  | 'started'
  | 'completed'
  | 'failed'
  | 'interrupted'
  | 'unknown'
  | 'skipped';

/** Confidence that a recovered side-effect actually landed. */
export type Confidence = 'high' | 'likely' | 'uncertain';

export interface RunJournalEntry {
  run_id: string;
  step_id: string;
  kind: StepKind;
  status: StepStatus;
  /** RFC3339 timestamp. */
  started_at: string;
  finished_at?: string | null;
  detail?: string | null;
}

export interface LedgerEntry {
  run_id: string;
  step_id: string;
  effect_kind: string;
  /** e.g. a commit SHA or branch name. */
  effect_ref?: string | null;
  confirmed: boolean;
  confidence: Confidence;
  recorded_at: string;
}

export interface ResumeReport {
  /** Completed write-steps that will be skipped (not re-run) on resume. */
  skipped: RunJournalEntry[];
  /** Steps started but never finished (app killed mid-step). */
  interrupted: RunJournalEntry[];
  /** Ledger effects that could not be confirmed and need human attention. */
  uncertain: LedgerEntry[];
}

export interface RunJournalResponse {
  journal: RunJournalEntry[];
  ledger: LedgerEntry[];
}

export interface ResumeOptions {
  skipCompletedWriteSteps: boolean;
}

/** Fetch the run journal + ledger for a session (#125). */
export async function getRunJournal(sessionId: string): Promise<RunJournalResponse> {
  return invoke<RunJournalResponse>('get_run_journal', { sessionId });
}

const WRITE_STEP_KINDS = new Set<StepKind>([
  'worker_spawn',
  'evaluator_spawn',
  'git_commit',
  'git_branch',
  'file_write',
]);

export function isResumeReportEmpty(report: ResumeReport): boolean {
  return (
    report.skipped.length === 0 &&
    report.interrupted.length === 0 &&
    report.uncertain.length === 0
  );
}

/**
 * Frontend preview of the backend resume classifier. The backend still performs
 * authoritative verification during `resume_session`; this read-only pass gives
 * the confirmation modal enough per-step context before the operator resumes.
 */
export function buildResumeReportFromJournal(response: RunJournalResponse): ResumeReport {
  const ledgerByStep = new Map(response.ledger.map((entry) => [entry.step_id, entry]));
  const report: ResumeReport = { skipped: [], interrupted: [], uncertain: [] };

  for (const entry of response.journal) {
    const ledger = ledgerByStep.get(entry.step_id);
    const hasUnconfirmedLedger = !!ledger && !ledger.confirmed;

    if (entry.status === 'completed' && WRITE_STEP_KINDS.has(entry.kind)) {
      report.skipped.push({ ...entry, status: 'skipped' });
      continue;
    }

    if (entry.status === 'started' || entry.status === 'unknown' || entry.status === 'interrupted') {
      report.interrupted.push({
        ...entry,
        status: hasUnconfirmedLedger ? 'unknown' : 'interrupted',
      });
      if (hasUnconfirmedLedger && ledger) {
        report.uncertain.push(ledger);
      }
    }
  }

  return report;
}

interface SessionsState {
  sessions: Session[];
  activeSessionId: string | null;
  loading: boolean;
  error: string | null;
}

function createSessionsStore() {
  const { subscribe, set, update } = writable<SessionsState>({
    sessions: [],
    activeSessionId: null,
    loading: false,
    error: null,
  });

  // Listen for session updates from backend
  listen<{ session: Session }>('session-update', (event) => {
    update((state) => {
      const idx = state.sessions.findIndex((s) => s.id === event.payload.session.id);
      if (idx >= 0) {
        state.sessions[idx] = event.payload.session;
      } else {
        state.sessions.push(event.payload.session);
      }
      return { ...state };
    });
  });

  function getState(): SessionsState {
    let current: SessionsState = {
      sessions: [],
      activeSessionId: null,
      loading: false,
      error: null,
    };
    subscribe((state) => (current = state))();
    return current;
  }

  return {
    subscribe,

    async loadSessions() {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const sessions = await invoke<Session[]>('list_sessions');
        update((state) => ({ ...state, sessions, loading: false }));
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
      }
    },

    async launchHive(projectPath: string, workerCount: number, command: string, prompt?: string) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const session = await invoke<Session>('launch_hive', {
          projectPath,
          workerCount,
          command,
          prompt,
        });
        update((state) => {
          // Only add if not already present (event listener may have added it)
          const exists = state.sessions.some((s) => s.id === session.id);
          return {
            ...state,
            sessions: exists ? state.sessions : [...state.sessions, session],
            activeSessionId: session.id,
            loading: false,
          };
        });
        return session;
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
        throw err;
      }
    },

    async launchSolo(config: SoloLaunchConfig) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        // Solo mode is implemented as a Hive session with 0 extra workers (just the Queen acting as the solo agent)
        const hiveConfig: HiveLaunchConfig = {
          project_path: config.projectPath,
          queen_config: {
            cli: config.cli,
            model: config.model,
            flags: [],
          },
          workers: [], // Empty workers list triggers solo mode in backend
          prompt: config.taskDescription,
          with_planning: false,
          with_evaluator: config.with_evaluator,
          evaluator_config: config.evaluator_config,
          qa_workers: config.qa_workers,
        };
        
        const session = await invoke<Session>('launch_hive_v2', { config: hiveConfig });
        update((state) => {
          const exists = state.sessions.some((s) => s.id === session.id);
          return {
            ...state,
            sessions: exists ? state.sessions : [...state.sessions, session],
            activeSessionId: session.id,
            loading: false,
          };
        });
        return session;
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
        throw err;
      }
    },

    async stopSession(sessionId: string) {
      try {
        await invoke('stop_session', { id: sessionId });
        update((state) => {
          const session = state.sessions.find((s) => s.id === sessionId);
          if (session) {
            session.state = 'Completed';
          }
          return { ...state };
        });
      } catch (err) {
        update((state) => ({ ...state, error: String(err) }));
        throw err;
      }
    },

    async closeSession(sessionId: string) {
      try {
        await invoke('close_session', { id: sessionId });
        update((state) => {
          const session = state.sessions.find((s) => s.id === sessionId);
          if (session) {
            session.state = 'Closed';
          }
          return { ...state };
        });
      } catch (err) {
        update((state) => ({ ...state, error: String(err) }));
        throw err;
      }
    },

    async launchHiveV2(config: HiveLaunchConfig) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const session = await invoke<Session>('launch_hive_v2', { config });
        update((state) => {
          const exists = state.sessions.some((s) => s.id === session.id);
          return {
            ...state,
            sessions: exists ? state.sessions : [...state.sessions, session],
            activeSessionId: session.id,
            loading: false,
          };
        });
        return session;
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
        throw err;
      }
    },

    async launchResearch(config: ResearchLaunchConfig) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const session = await invoke<Session>('launch_research', { config });
        update((state) => {
          const exists = state.sessions.some((s) => s.id === session.id);
          return {
            ...state,
            sessions: exists ? state.sessions : [...state.sessions, session],
            activeSessionId: session.id,
            loading: false,
          };
        });
        return session;
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
        throw err;
      }
    },

    async launchSwarm(config: SwarmLaunchConfig) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const session = await invoke<Session>('launch_swarm', { config });
        update((state) => {
          const exists = state.sessions.some((s) => s.id === session.id);
          return {
            ...state,
            sessions: exists ? state.sessions : [...state.sessions, session],
            activeSessionId: session.id,
            loading: false,
          };
        });
        return session;
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
        throw err;
      }
    },

    async launchFusion(config: FusionLaunchConfig) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const session = await invoke<Session>('launch_fusion', { config });
        update((state) => {
          const exists = state.sessions.some((s) => s.id === session.id);
          return {
            ...state,
            sessions: exists ? state.sessions : [...state.sessions, session],
            activeSessionId: session.id,
            loading: false,
          };
        });
        return session;
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
        throw err;
      }
    },

    async launchDebate(config: DebateLaunchConfig) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const session = await invoke<Session>('launch_debate', { config });
        update((state) => {
          const exists = state.sessions.some((s) => s.id === session.id);
          return {
            ...state,
            sessions: exists ? state.sessions : [...state.sessions, session],
            activeSessionId: session.id,
            loading: false,
          };
        });
        return session;
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
        throw err;
      }
    },

    setActiveSession(sessionId: string | null) {
      update((state) => ({ ...state, activeSessionId: sessionId }));
      // Route navigation-state persistence at this session and (re)start the
      // snapshot-hydrate + watermark poll loop. On switch the store resets to 0.
      ui.setPersistSession(sessionId);
      if (sessionId) {
        applicationState.start(sessionId);
      } else {
        applicationState.stop();
      }
    },

    removeSession(sessionId: string) {
      update((state) => ({
        ...state,
        sessions: state.sessions.filter((s) => s.id !== sessionId),
        activeSessionId: state.activeSessionId === sessionId ? null : state.activeSessionId,
      }));
    },

    async continueAfterPlanning(sessionId: string) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const session = await invoke<Session>('continue_after_planning', { sessionId });
        update((state) => {
          const idx = state.sessions.findIndex((s) => s.id === session.id);
          if (idx >= 0) {
            state.sessions[idx] = session;
          }
          return { ...state, loading: false };
        });
        return session;
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
        throw err;
      }
    },

    async markPlanReady(sessionId: string) {
      try {
        await invoke('mark_plan_ready', { sessionId });
        update((state) => {
          const session = state.sessions.find((s) => s.id === sessionId);
          if (session) {
            session.state = 'PlanReady';
          }
          return { ...state };
        });
      } catch (err) {
        update((state) => ({ ...state, error: String(err) }));
        throw err;
      }
    },

    async getResumeReport(sessionId: string): Promise<ResumeReport | null> {
      const loadedSession = getState().sessions.find((session) => session.id === sessionId);
      if (loadedSession?.resume_report) {
        return loadedSession.resume_report;
      }

      const response = await getRunJournal(sessionId);
      const report = buildResumeReportFromJournal(response);
      return isResumeReportEmpty(report) ? null : report;
    },

    async resumeSession(sessionId: string, _options?: ResumeOptions) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const session = await invoke<Session>('resume_session', { sessionId });
        update((state) => {
          const exists = state.sessions.some((s) => s.id === session.id);
          return {
            ...state,
            sessions: exists ? state.sessions : [...state.sessions, session],
            activeSessionId: session.id,
            loading: false,
          };
        });
        return session;
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
        throw err;
      }
    },

    async applyFusionWinner(sessionId: string, variantName: string) {
      try {
        await invoke('apply_fusion_winner', { sessionId, variantName });
      } catch (err) {
        update((state) => ({ ...state, error: String(err) }));
        throw err;
      }
    },

    async updateSessionMetadata(id: string, name?: string | null, color?: string | null) {
      try {
        const session = await invoke<Session>('update_session_metadata', { id, name, color });
        update((state) => {
          const idx = state.sessions.findIndex((s) => s.id === session.id);
          if (idx >= 0) {
            state.sessions[idx] = session;
          }
          return { ...state };
        });
        return session;
      } catch (err) {
        update((state) => ({ ...state, error: String(err) }));
        throw err;
      }
    },
  };
}

export const sessions = createSessionsStore();

export const activeSession = derived(sessions, ($sessions) =>
  $sessions.sessions.find((s) => s.id === $sessions.activeSessionId) ?? null
);

export const activeAgents = derived(activeSession, ($activeSession) =>
  $activeSession?.agents ?? []
);

export interface BranchInfo {
  name: string;
  short_hash: string;
  is_current: boolean;
}

export const currentBranch = writable<string>('');
export const availableBranches = writable<BranchInfo[]>([]);
