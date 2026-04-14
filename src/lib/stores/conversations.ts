import { writable } from 'svelte/store';
import { listen } from '@tauri-apps/api/event';
import { apiUrl } from '$lib/config';

export interface ConversationMessage {
  timestamp: string;
  from: string;
  content: string;
  agent_id?: string;
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

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function createConversationStore() {
  const { subscribe, update } = writable<ConversationState>({
    messages: [],
    loading: false,
    error: null,
    selectedAgent: null,
    sessionId: null,
  });

  // Listen for real-time conversation messages from Tauri
  listen<ConversationMessage>('conversation-message', (event) => {
    update((state) => {
      // CONTRACT: only push if it belongs to selected agent (or no agent_id in payload, but we fixed that)
      if (event.payload.agent_id && state.selectedAgent !== event.payload.agent_id) {
        return state;
      }
      return {
        ...state,
        messages: [...state.messages, event.payload],
      };
    });
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
        let url = apiUrl(`/api/sessions/${sessionId}/conversations/${agentId}`);
        if (since) url += `?since=${encodeURIComponent(since)}`;
        const resp = await fetch(url);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const data = await resp.json();
        const messages: ConversationMessage[] = data.messages ?? [];
        
        update((state) => {
           // If we are appending (using 'since'), don't overwrite
           const newMessages = since 
             ? [...state.messages, ...messages] 
             : messages;
           return {
             ...state,
             messages: newMessages,
             loading: false,
             sessionId,
             selectedAgent: agentId,
           };
        });
      } catch (err) {
        update((state) => ({ ...state, loading: false, error: String(err) }));
      }
    },

    async sendMessage(sessionId: string, agentId: string, from: string, content: string) {
      try {
        const resp = await fetch(
          apiUrl(`/api/sessions/${sessionId}/conversations/${agentId}/append`),
          {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ from, content }),
          }
        );
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        // No need to reload - Tauri event will push it (and poll will catch up if missed)
      } catch (err) {
        update((state) => ({ ...state, error: String(err) }));
      }
    },

    // CONTRACT: Fallback poll in case Tauri event is lost
    async pollMessages() {
      const state = getState();
      if (!state.sessionId || !state.selectedAgent) return;
      
      const lastMsg = state.messages[state.messages.length - 1];
      const since = lastMsg?.timestamp;
      
      await this.loadConversation(state.sessionId, state.selectedAgent, since);
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

    async loadHeartbeats(sessionId: string): Promise<void> {
      try {
        let resp = await fetch(apiUrl('/api/sessions/active'));
        if (resp.status === 404) {
          await delay(1000);
          resp = await fetch(apiUrl('/api/sessions/active'));
        }

        if (!resp.ok) {
          throw new Error(`HTTP ${resp.status}`);
        }

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
