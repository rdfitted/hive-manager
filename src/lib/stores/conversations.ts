import { writable } from 'svelte/store';
import { listen } from '@tauri-apps/api/event';
import { apiUrl } from '$lib/config';

export interface ConversationMessage {
  id?: string;
  timestamp: string;
  from: string;
  content: string;
  agent_id?: string;
  session_id?: string;
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
  staleAgents: Set<string>; // Agents with heartbeat >3min old
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function hashContent(value: string): string {
  let hash = 0;
  for (let i = 0; i < value.length; i += 1) {
    hash = (hash * 31 + value.charCodeAt(i)) >>> 0;
  }

  return hash.toString(16);
}

function conversationMessageSignature(message: ConversationMessage): string {
  return [
    message.timestamp,
    message.from,
    message.agent_id ?? '',
    hashContent(message.content),
  ].join('|');
}

function dedupeConversationMessages(messages: ConversationMessage[]): ConversationMessage[] {
  const seenIds = new Set<string>();
  const seenSignatures = new Set<string>();
  const deduped: ConversationMessage[] = [];

  for (const message of messages) {
    const signature = conversationMessageSignature(message);
    const id = message.id;
    if ((id && seenIds.has(id)) || seenSignatures.has(signature)) {
      continue;
    }

    if (id) {
      seenIds.add(id);
    }
    seenSignatures.add(signature);
    deduped.push(message);
  }

  return deduped;
}

function mergeConversationMessages(
  existing: ConversationMessage[],
  incoming: ConversationMessage[],
): ConversationMessage[] {
  return dedupeConversationMessages([...existing, ...incoming]);
}

function createConversationStore() {
  let activeConversationRequest = 0;
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
      if (!state.sessionId || !state.selectedAgent) {
        return state;
      }

      if (event.payload.session_id !== state.sessionId) {
        return state;
      }

      if (event.payload.agent_id !== state.selectedAgent) {
        return state;
      }

      return {
        ...state,
        messages: mergeConversationMessages(state.messages, [event.payload]),
      };
    });
  });

  return {
    subscribe,

    selectAgent(agentId: string | null) {
      activeConversationRequest += 1;
      update((state) => ({
        ...state,
        selectedAgent: agentId,
        messages: [],
        loading: false,
        error: null,
      }));
    },

    setSessionId(sessionId: string | null) {
      activeConversationRequest += 1;
      update((state) => ({
        ...state,
        sessionId,
        messages: [],
        selectedAgent: null,
        loading: false,
        error: null,
      }));
    },

    async loadConversation(sessionId: string, agentId: string, since?: string) {
      const requestToken = ++activeConversationRequest;
      update((state) => ({ ...state, loading: true, error: null }));
      try {
        let url = apiUrl(`/api/sessions/${sessionId}/conversations/${agentId}`);
        if (since) url += `?since=${encodeURIComponent(since)}`;
        const resp = await fetch(url);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const data = await resp.json();
        const messages: ConversationMessage[] = data.messages ?? [];
        
        update((state) => {
          if (requestToken !== activeConversationRequest) {
            return state;
          }

          if (state.sessionId !== sessionId || state.selectedAgent !== agentId) {
            return { ...state, loading: false };
          }

          const newMessages = since
            ? mergeConversationMessages(state.messages, messages)
            : dedupeConversationMessages(messages);

          return {
            ...state,
            messages: newMessages,
            loading: false,
            sessionId,
            selectedAgent: agentId,
          };
        });
      } catch (err) {
        update((state) => {
          if (requestToken !== activeConversationRequest) {
            return state;
          }

          return { ...state, loading: false, error: String(err) };
        });
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
        await this.pollMessages();
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

// Staleness threshold: 3 minutes in milliseconds
const STALENESS_THRESHOLD_MS = 3 * 60 * 1000;

function isHeartbeatStale(timestamp: string): boolean {
  if (!timestamp) return true;
  const hbTime = new Date(timestamp).getTime();
  if (isNaN(hbTime)) return true;
  return Date.now() - hbTime > STALENESS_THRESHOLD_MS;
}

function createHeartbeatStore() {
  const { subscribe, update } = writable<HeartbeatState>({
    agents: {},
    stalledAgents: new Set(),
    staleAgents: new Set(),
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
        // Backend returns { sessions: [...] }; pick matching session (or first if only one).
        const sessions: Array<{ id: string; agents?: Array<{ id?: string; agent_id?: string; status?: string; summary?: string; last_activity?: string }> }> =
          Array.isArray(data?.sessions) ? data.sessions : [];
        const session = sessions.find((s) => s.id === sessionId) ?? (sessions.length === 1 ? sessions[0] : undefined);
        if (session?.agents) {
          const agents: Record<string, HeartbeatInfo> = {};
          const staleAgents = new Set<string>();
          for (const agent of session.agents) {
            const id = agent.id || agent.agent_id;
            if (!id) continue;
            const timestamp = agent.last_activity ?? '';
            agents[id] = {
              agent_id: id,
              status: agent.status || 'unknown',
              summary: agent.summary || '',
              timestamp,
            };
            // Staleness: missing last_activity or >3min old = stale.
            if (!timestamp || isHeartbeatStale(timestamp)) {
              staleAgents.add(id);
            }
          }
          update((state) => ({ ...state, agents, staleAgents }));
        }
      } catch {
        // Silently ignore heartbeat fetch errors
      }
    },
  };
}

export const conversationStore = createConversationStore();
export const heartbeatStore = createHeartbeatStore();
