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
  /**
   * Native tool-render envelope (issue #127). Optional + backward-compatible:
   * old messages and the Tauri 'conversation-message' payload deserialize
   * unchanged. Dedupe keeps the original identity and enriches duplicate rows
   * with their persisted or live envelope instead of discarding the widget.
   * `renderer` is a plain string hint; `data` is the structured payload.
   */
  renderer?: string;
  data?: unknown;
}

export interface HeartbeatInfo {
  agent_id: string;
  status: string;
  summary: string;
  timestamp: string;
  work_state?: string;
  latest_input_at?: string;
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
  agentsBySession: Record<string, Record<string, HeartbeatInfo>>;
  staleAgentsBySession: Record<string, Set<string>>;
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
  const seenIds = new Map<string, number>();
  const seenSignatures = new Map<string, number>();
  const bodyMatches = new Map<string, number[]>();
  const deduped: ConversationMessage[] = [];

  for (const message of messages) {
    const signature = conversationMessageSignature(message);
    const bodySignature = conversationMessageSignature({ ...message, agent_id: undefined });
    const id = message.id;
    let index = (id ? seenIds.get(id) : undefined) ?? seenSignatures.get(signature);
    // GET omits routing fields; live messages carry them. In the selected
    // conversation these are the same row. Two explicit agent IDs stay distinct.
    index ??= bodyMatches.get(bodySignature)?.find((candidate) =>
      !message.agent_id || !deduped[candidate].agent_id);
    if (index === undefined) {
      index = deduped.length;
      deduped.push(message);
      bodyMatches.set(bodySignature, [...(bodyMatches.get(bodySignature) ?? []), index]);
    } else {
      const previous = deduped[index];
      if (previous.content === message.content) {
        deduped[index] = {
          ...previous,
          ...(!previous.id && id ? { id } : {}),
          ...(!previous.agent_id && message.agent_id ? { agent_id: message.agent_id } : {}),
          ...(!previous.session_id && message.session_id ? { session_id: message.session_id } : {}),
          ...(message.renderer ? { renderer: message.renderer } : {}),
          ...(message.data !== undefined && message.data !== null ? { data: message.data } : {}),
        };
      }
    }

    if (id) {
      seenIds.set(id, index);
    }
    seenSignatures.set(signature, index);
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
  let liveDuringRequest: ConversationMessage[] | null = null;
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

      liveDuringRequest?.push(event.payload);
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
      liveDuringRequest = null;
      update((state) => ({
        ...state,
        selectedAgent: agentId,
        messages: [],
        loading: false,
        error: null,
      }));
    },

    setSessionId(sessionId: string | null) {
      if (getState().sessionId === sessionId) return;
      activeConversationRequest += 1;
      liveDuringRequest = null;
      update((state) => ({
        ...state,
        sessionId,
        messages: [],
        selectedAgent: null,
        loading: false,
        error: null,
      }));
    },

    async loadConversation(
      sessionId: string,
      agentId: string,
      since?: string,
      options?: { silent?: boolean },
    ) {
      const requestToken = ++activeConversationRequest;
      const liveMessages: ConversationMessage[] = [];
      liveDuringRequest = liveMessages;
      update((state) => ({
        ...state,
        loading: options?.silent ? state.loading : true,
        error: options?.silent ? state.error : null,
      }));
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
            : mergeConversationMessages(messages, liveMessages);

          return {
            ...state,
            messages: newMessages,
            loading: false,
            error: null,
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
      } finally {
        if (requestToken === activeConversationRequest) liveDuringRequest = null;
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
      
      await this.loadConversation(state.sessionId, state.selectedAgent, since, { silent: true });
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

function isHeartbeatStale(timestamp: string | undefined, injectedAt?: string): boolean {
  const activity = [timestamp, injectedAt].map(value => Date.parse(value ?? '')).filter(Number.isFinite);
  return activity.length > 0 && Date.now() - Math.max(...activity) > STALENESS_THRESHOLD_MS;
}

function createHeartbeatStore() {
  const { subscribe, update } = writable<HeartbeatState>({
    agents: {},
    agentsBySession: {},
    staleAgentsBySession: {},
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

    async loadHeartbeats(sessionId: string | null): Promise<void> {
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
        // One response supplies every sidebar row; retain the requested view for existing consumers.
        const sessions: Array<{ id: string; agents?: Array<{ id?: string; agent_id?: string; status?: string; summary?: string; last_activity?: string; work_state?: string; awaiting_injected_turn_at?: string; latest_input_at?: string }> }> =
          Array.isArray(data?.sessions) ? data.sessions : [];
        const agentsBySession: HeartbeatState['agentsBySession'] = {};
        const staleAgentsBySession: HeartbeatState['staleAgentsBySession'] = {};
        for (const session of sessions) {
          const agents: Record<string, HeartbeatInfo> = {};
          const staleAgents = new Set<string>();
          for (const agent of session.agents ?? []) {
            const id = agent.id || agent.agent_id;
            if (!id) continue;
            const timestamp = agent.last_activity ?? '';
            agents[id] = {
              agent_id: id,
              status: agent.status || 'unknown',
              summary: agent.summary || '',
              timestamp,
              work_state: agent.work_state,
              latest_input_at: agent.latest_input_at,
            };
            if (agent.work_state !== 'finished' && agent.work_state !== 'retired'
              && isHeartbeatStale(timestamp, agent.latest_input_at ?? agent.awaiting_injected_turn_at)) {
              staleAgents.add(id);
            }
          }
          agentsBySession[session.id] = agents;
          staleAgentsBySession[session.id] = staleAgents;
        }
        update((state) => ({ ...state, agentsBySession, staleAgentsBySession,
          agents: sessionId ? agentsBySession[sessionId] ?? {} : {},
          staleAgents: sessionId ? staleAgentsBySession[sessionId] ?? new Set() : new Set(),
        }));
      } catch {
        // Silently ignore heartbeat fetch errors
      }
    },
  };
}

export const conversationStore = createConversationStore();
export const heartbeatStore = createHeartbeatStore();
