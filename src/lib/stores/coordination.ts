import { writable, derived } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type MessageType = 'Task' | 'Progress' | 'Completion' | 'Error' | 'System' | 'Judge';

export interface CoordinationMessage {
  id: string;
  timestamp: string;
  from: string;
  to: string;
  content: string;
  message_type: MessageType;
}

export interface WorkerStateInfo {
  id: string;
  role: WorkerRole;
  cli: string;
  status: string;
  current_task: string | null;
  last_update: string;
}

export interface WorkerRole {
  role_type: string;
  label: string;
  default_cli: string;
  prompt_template: string | null;
}

export interface QueenInjectRequest {
  session_id: string;
  queen_id: string;
  target_worker_id: string;
  message: string;
}

export interface AddWorkerRequest {
  session_id: string;
  config: {
    cli: string;
    model?: string;
    flags: string[];
    label?: string;
    role?: WorkerRole;
    initial_prompt?: string;
  };
  role: WorkerRole;
  parent_id?: string;
}

interface CoordinationState {
  log: CoordinationMessage[];
  workers: WorkerStateInfo[];
  fusionState: {
    completedVariants: string[];
    judgeReport: string | null;
    evaluationReady: boolean;
  };
  loading: boolean;
  error: string | null;
  sessionId: string | null;
}

function createCoordinationStore() {
  const { subscribe, set, update } = writable<CoordinationState>({
    log: [],
    workers: [],
    fusionState: {
      completedVariants: [],
      judgeReport: null,
      evaluationReady: false,
    },
    loading: false,
    error: null,
    sessionId: null,
  });

  // Listen for coordination messages from backend
  listen<CoordinationMessage>('coordination-message', (event) => {
    update((state) => {
      // Add new message to log
      const exists = state.log.some((m) => m.id === event.payload.id);
      if (!exists) {
        return {
          ...state,
          log: [...state.log, event.payload],
        };
      }
      return state;
    });
  });

  // Listen for fusion variant completion
  listen<{ variant: string }>('fusion-variant-completed', (event) => {
    update((state) => ({
      ...state,
      fusionState: {
        ...state.fusionState,
        completedVariants: [...state.fusionState.completedVariants, event.payload.variant],
      },
    }));
  });

  // Listen for judge evaluation ready
  listen<{ report: string }>('judge-evaluation-ready', (event) => {
    update((state) => ({
      ...state,
      fusionState: {
        ...state.fusionState,
        judgeReport: event.payload.report,
        evaluationReady: true,
      },
    }));
  });

  return {
    subscribe,

    setSessionId(sessionId: string | null) {
      update((state) => ({
        ...state,
        sessionId,
        log: sessionId === state.sessionId ? state.log : [],
        workers: sessionId === state.sessionId ? state.workers : [],
        fusionState: sessionId === state.sessionId ? state.fusionState : {
          completedVariants: [],
          judgeReport: null,
          evaluationReady: false,
        },
      }));
    },

    async loadLog(sessionId: string, limit?: number) {
      update((state) => ({ ...state, loading: true, error: null, sessionId }));
      try {
        const log = await invoke<CoordinationMessage[]>('get_coordination_log', {
          sessionId,
          limit,
        });
        update((state) => ({
          ...state,
          log,
          loading: false,
        }));
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
      }
    },

    async loadWorkers(sessionId: string) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const workers = await invoke<WorkerStateInfo[]>('get_workers_state', {
          sessionId,
        });
        update((state) => ({
          ...state,
          workers,
          loading: false,
        }));
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
      }
    },

    async queenInject(
      sessionId: string,
      queenId: string,
      targetWorkerId: string,
      message: string
    ) {
      const request: QueenInjectRequest = {
        session_id: sessionId,
        queen_id: queenId,
        target_worker_id: targetWorkerId,
        message,
      };

      try {
        await invoke('queen_inject', { request });
      } catch (err) {
        update((state) => ({ ...state, error: String(err) }));
        throw err;
      }
    },

    async addWorker(request: AddWorkerRequest) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        const agentInfo = await invoke('add_worker_to_session', { request });
        update((state) => ({ ...state, loading: false }));
        return agentInfo;
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
        throw err;
      }
    },

    async assignTask(
      sessionId: string,
      queenId: string,
      workerId: string,
      task: string,
      planTaskId?: string | null
    ) {
      try {
        const payload: Record<string, unknown> = {
          sessionId,
          queenId,
          workerId,
          task,
        };
        if (planTaskId) {
          payload.planTaskId = planTaskId;
        }
        await invoke('assign_task', {
          ...payload,
        });
      } catch (err) {
        update((state) => ({ ...state, error: String(err) }));
        throw err;
      }
    },

    clearLog() {
      update((state) => ({ ...state, log: [] }));
    },

    clearError() {
      update((state) => ({ ...state, error: null }));
    },
  };
}

export const coordination = createCoordinationStore();

// Derived store for messages grouped by sender type
export const messagesByType = derived(coordination, ($coordination) => {
  const queen: CoordinationMessage[] = [];
  const workers: CoordinationMessage[] = [];
  const system: CoordinationMessage[] = [];

  for (const msg of $coordination.log) {
    if (msg.from === 'SYSTEM') {
      system.push(msg);
    } else if (msg.from === 'QUEEN') {
      queen.push(msg);
    } else {
      workers.push(msg);
    }
  }

  return { queen, workers, system };
});

// Derived store for active workers count
export const activeWorkersCount = derived(coordination, ($coordination) =>
  $coordination.workers.filter((w) => w.status === 'Running').length
);
