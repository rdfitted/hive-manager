import { writable, derived } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type AgentRole =
  | 'Queen'
  | { Planner: { index: number } }
  | { Worker: { index: number; parent: string | null } }
  | { Fusion: { variant: string } };

export type AgentStatus = 'Starting' | 'Running' | 'WaitingForInput' | 'Completed' | { Error: string };

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
  project_path: string;
  queen_config: AgentConfig;
  workers: AgentConfig[];
  prompt?: string;
}

export interface PlannerConfig {
  config: AgentConfig;
  domain: string;
  workers: AgentConfig[];
}

export interface SwarmLaunchConfig {
  project_path: string;
  queen_config: AgentConfig;
  planners: PlannerConfig[];
  prompt?: string;
}

export interface Session {
  id: string;
  session_type: { Hive: { worker_count: number } } | { Swarm: { planner_count: number } } | { Fusion: { variants: string[] } };
  project_path: string;
  state: 'Starting' | 'Running' | 'Paused' | 'Completed' | { Failed: string };
  created_at: string;
  agents: AgentInfo[];
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

    setActiveSession(sessionId: string | null) {
      update((state) => ({ ...state, activeSessionId: sessionId }));
    },

    removeSession(sessionId: string) {
      update((state) => ({
        ...state,
        sessions: state.sessions.filter((s) => s.id !== sessionId),
        activeSessionId: state.activeSessionId === sessionId ? null : state.activeSessionId,
      }));
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
