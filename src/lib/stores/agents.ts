import { writable } from 'svelte/store';
import { listen } from '@tauri-apps/api/event';
import type { Agent } from '../types/domain';
import { apiUrl } from '$lib/config';

interface AgentsState {
    agents: Record<string, Agent>; // agent_id -> Agent
    loading: boolean;
    error: string | null;
}

/// Durable run-queue (#126) status, normalized from the backend's snake_case serde enum.
export type QueueStatus = 'queued' | 'running' | 'finalized' | 'failed';

export interface QueueRow {
    id: string;
    task_id: string | null;
    session_id: string;
    worker_id: string;
    role_type: string;
    cli: string;
    status: QueueStatus;
    payload: unknown;
    attempts: number;
    continuation_count: number;
    no_progress_count: number;
    last_status: string | null;
    heartbeat_at: number | null;
    created_at: number;
    updated_at: number;
}

export interface QueueSnapshot {
    queued: number;
    running: number;
    finalized: number;
    failed: number;
    rows: QueueRow[];
}

interface QueueState {
    sessionId: string | null;
    snapshot: QueueSnapshot | null;
    loading: boolean;
    error: string | null;
}

const KNOWN_QUEUE_STATUSES: readonly QueueStatus[] = ['queued', 'running', 'finalized', 'failed'];

/// Normalize an arbitrary status string into a known QueueStatus, mirroring the
/// serde-enum-normalization pattern used by the other stores. Unknown values fall back
/// to 'queued' so the UI never renders a raw/unknown tag.
function normalizeQueueStatus(raw: unknown): QueueStatus {
    const value = typeof raw === 'string' ? raw.toLowerCase() : '';
    return (KNOWN_QUEUE_STATUSES as readonly string[]).includes(value)
        ? (value as QueueStatus)
        : 'queued';
}

function normalizeSnapshot(raw: QueueSnapshot): QueueSnapshot {
    return {
        ...raw,
        rows: (raw.rows ?? []).map((row) => ({ ...row, status: normalizeQueueStatus(row.status) })),
    };
}

function createQueueStore() {
    const { subscribe, set, update } = writable<QueueState>({
        sessionId: null,
        snapshot: null,
        loading: false,
        error: null,
    });

    // Refresh whenever the backend's 30s maintenance pass (or a queue mutation) fires.
    void listen('queue-updated', () => {
        let activeSessionId: string | null = null;
        const unsub = subscribe((state) => {
            activeSessionId = state.sessionId;
        });
        unsub();
        if (activeSessionId) {
            void queueStore.fetchQueue(activeSessionId);
        }
    });

    return {
        subscribe,

        async fetchQueue(sessionId: string) {
            update((state) => ({ ...state, sessionId, loading: true, error: null }));
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/queue`));
                if (!response.ok) {
                    throw new Error(`Failed to fetch queue: ${response.statusText}`);
                }
                const raw: QueueSnapshot = await response.json();
                update((state) => ({
                    ...state,
                    snapshot: normalizeSnapshot(raw),
                    loading: false,
                }));
            } catch (err) {
                update((state) => ({ ...state, loading: false, error: (err as Error).message }));
            }
        },

        reset() {
            set({ sessionId: null, snapshot: null, loading: false, error: null });
        },
    };
}

export const queueStore = createQueueStore();

function createAgentsStore() {
    const { subscribe, set, update } = writable<AgentsState>({
        agents: {},
        loading: false,
        error: null,
    });

    return {
        subscribe,

        async fetchAgents(sessionId: string, cellId: string) {
            update(state => ({ ...state, loading: true, error: null }));
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/cells/${cellId}/agents`));
                if (!response.ok) throw new Error(`Failed to fetch agents: ${response.statusText}`);
                const agents: Agent[] = await response.json();
                
                update(state => {
                    const newAgents = { ...state.agents };
                    agents.forEach(agent => {
                        newAgents[agent.id] = agent;
                    });
                    return { ...state, agents: newAgents, loading: false };
                });
            } catch (err) {
                update(state => ({ ...state, loading: false, error: (err as Error).message }));
            }
        },

        updateAgent(agent: Agent) {
            update(state => ({
                ...state,
                agents: { ...state.agents, [agent.id]: agent }
            }));
        }
    };
}

export const agents = createAgentsStore();
