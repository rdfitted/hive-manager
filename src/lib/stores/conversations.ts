import { writable, derived } from 'svelte/store';
import { listen } from '@tauri-apps/api/event';

const API_BASE = 'http://localhost:18800';

export interface ConversationMessage {
  timestamp: string;
  from: string;
  content: string;
}

export interface HeartbeatInfo {
  agent_id: string;
  status: string;
  summary: string;
  timestamp: string;
}

interface ConversationState {
  messages: ConversationMessage[];
  loading: boolean;
  error: string | null;
  selectedAgent: string | null;
  sessionId: string | null;
}

interface HeartbeatState {
  agents: Record<string, HeartbeatInfo>;
  stalledAgents: Set<string>;
}

function createConversationStore() {
  const { subscribe, set, update } = writable<ConversationState>({
    messages: [],
    loading: false,
    error: null,
    selectedAgent: null,
    sessionId: null,
  });

  // Listen for real-time conversation messages from Tauri
  listen<ConversationMessage>('conversation-message', (event) => {
    update((state) => ({
      ...state,
      messages: [...state.messages, event.payload],
    }));
  });

  return {
    subscribe,

    selectAgent(agentId: string | null) {
      update((state) => ({ ...state, selectedAgent: agentId, messages: [] }));
    },

    setSessionId(sessionId: string | null) {
      update((state) => ({
        ...state,
        sessionId,
        messages: [],
        selectedAgent: null,
      }));
    },

    async loadConversation(sessionId: string, agentId: string, since?: string) {
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        let url = `${API_BASE}/api/sessions/${sessionId}/conversations/${agentId}`;
        if (since) url += `?since=${encodeURIComponent(since)}`;
        const resp = await fetch(url);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const data = await resp.json();
        const messages: ConversationMessage[] = data.messages ?? [];
        update((state) => ({
          ...state,
          messages,
          loading: false,
          sessionId,
          selectedAgent: agentId,
        }));
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
      }
    },

    async sendMessage(sessionId: string, agentId: string, from: string, content: string) {
      try {
        const resp = await fetch(
          `${API_BASE}/api/sessions/${sessionId}/conversations/${agentId}/append`,
          {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ from, content }),
          }
        );
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        // Reload to get the appended message
        const state = getState();
        if (state.sessionId && state.selectedAgent) {
          await this.loadConversation(state.sessionId, state.selectedAgent);
        }
      } catch (err) {
        update((state) => ({ ...state, error: String(err) }));
      }
    },

    clearError() {
      update((state) => ({ ...state, error: null }));
    },
  };

  function getState(): ConversationState {
    let current: ConversationState = {
      messages: [],
      loading: false,
      error: null,
      selectedAgent: null,
      sessionId: null,
    };
    subscribe((s) => (current = s))();
    return current;
  }
}

function createHeartbeatStore() {
  const { subscribe, update } = writable<HeartbeatState>({
    agents: {},
    stalledAgents: new Set(),
  });

  // Listen for stall/recovery events
  listen<{ agent_id: string }>('agent-stalled', (event) => {
    update((state) => {
      const stalledAgents = new Set(state.stalledAgents);
      stalledAgents.add(event.payload.agent_id);
      return { ...state, stalledAgents };
    });
  });

  listen<{ agent_id: string }>('agent-recovered', (event) => {
    update((state) => {
      const stalledAgents = new Set(state.stalledAgents);
      stalledAgents.delete(event.payload.agent_id);
      return { ...state, stalledAgents };
    });
  });

  return {
    subscribe,

    async loadHeartbeats(sessionId: string) {
      try {
        const resp = await fetch(`${API_BASE}/api/sessions/active`);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const data = await resp.json();
        // Extract agent heartbeats for this session
        const session = Array.isArray(data)
          ? data.find((s: { id: string }) => s.id === sessionId)
          : data;
        if (session?.agents) {
          const agents: Record<string, HeartbeatInfo> = {};
          for (const agent of session.agents) {
            agents[agent.agent_id || agent.id] = {
              agent_id: agent.agent_id || agent.id,
              status: agent.status || 'unknown',
              summary: agent.summary || '',
              timestamp: agent.timestamp || agent.last_update || new Date().toISOString(),
            };
          }
          update((state) => ({ ...state, agents }));
        }
      } catch {
        // Silently ignore heartbeat fetch errors
      }
    },
  };
}

export const conversationStore = createConversationStore();
export const heartbeatStore = createHeartbeatStore();
