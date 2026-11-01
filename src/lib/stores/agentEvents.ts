import { writable, get } from 'svelte/store';
import { apiUrl } from '$lib/config';

export interface AgentEvent {
  agent_id: string;
  cursor: string;
  ts: string;
  kind: 'user_message' | 'assistant_delta' | 'assistant_message' | 'tool_start' | 'tool_result'
    | 'turn_start' | 'turn_end' | 'status' | 'error';
  source_id: string | null;
  truncated: boolean;
  payload: Record<string, unknown>;
}

export interface AgentFeed {
  agent_id: string;
  capabilities: {
    has_terminal: boolean;
    can_interrupt: boolean;
    supports_approvals: boolean;
    event_source: 'none' | 'transcript' | 'protocol';
  };
}

interface HistoryPage {
  agent_id: string;
  events: AgentEvent[];
  next_before: string | null;
  latest_cursor: string | null;
  has_more: boolean;
  reset: boolean;
}

export interface AgentHistory {
  events: AgentEvent[];
  before: string | null;
  hasMore: boolean;
  loading: boolean;
  error: string | null;
}

export interface AgentEventsState {
  sessionId: string | null;
  feeds: Record<string, AgentFeed>;
  histories: Record<string, AgentHistory>;
  connected: boolean;
  error: string | null;
}

const emptyHistory = (): AgentHistory => ({ events: [], before: null, hasMore: false, loading: false, error: null });
const emptyState = (sessionId: string | null = null): AgentEventsState => ({ sessionId, feeds: {}, histories: {}, connected: false, error: null });

/** Cursors are opaque. Preserve server order and compare only for equality. */
function unique(events: AgentEvent[]): AgentEvent[] {
  const seen = new Set<string>();
  return events.filter((event) => !seen.has(event.cursor) && !!seen.add(event.cursor));
}

/** A reset's SSE replay may begin before the HTTP tail and continue after it. */
function mergeTail(incoming: AgentEvent[], live: AgentEvent[]): AgentEvent[] {
  const cursors = new Set(incoming.map((event) => event.cursor));
  const overlap = live.findIndex((event) => cursors.has(event.cursor));
  return overlap < 0 ? unique([...incoming, ...live])
    : unique([...live.slice(0, overlap), ...incoming, ...live.slice(overlap)]);
}

function checkpoint(cursors: Record<string, string>): string {
  const bytes = new TextEncoder().encode(JSON.stringify(cursors));
  return btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(''))
    .replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}

/** One controller per active session, shared by every Focus/Grid pane. */
export function createAgentEventsStore() {
  const state = writable<AgentEventsState>(emptyState());
  let source: EventSource | null = null;
  let abort: AbortController | null = null;
  let generation = 0;
  let rosterTimer: ReturnType<typeof setInterval> | undefined;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  const revisions = new Map<string, number>();
  let rosterLoading = false;

  function changeHistory(agentId: string, change: (history: AgentHistory) => AgentHistory) {
    state.update((value) => ({ ...value, histories: { ...value.histories, [agentId]: change(value.histories[agentId] ?? emptyHistory()) } }));
  }

  async function read<T>(path: string, signal: AbortSignal): Promise<T> {
    const response = await fetch(apiUrl(path), { signal });
    if (!response.ok) throw new Error(`Agent feed request failed (${response.status})`);
    return response.json();
  }

  async function history(agentId: string, older = false, afterReset = false) {
    const snapshot = get(state);
    const sid = snapshot.sessionId;
    const controller = abort;
    const run = generation;
    const revision = revisions.get(agentId) ?? 0;
    const current = snapshot.histories[agentId];
    if (!sid || !controller || current?.loading || (older && !current?.hasMore)) return;
    changeHistory(agentId, (h) => ({ ...h, loading: true, error: null }));
    const query = new URLSearchParams({ limit: '100' });
    if (older && current?.before) query.set('before', current.before);
    try {
      const page = await read<HistoryPage>(`/api/sessions/${encodeURIComponent(sid)}/agents/${encodeURIComponent(agentId)}/events?${query}`, controller.signal);
      if (run !== generation || revision !== (revisions.get(agentId) ?? 0)) return;
      if (page.agent_id !== agentId) throw new Error('Agent history identity mismatch');
      const incoming = page.events.filter((event) => event.agent_id === agentId);
      changeHistory(agentId, (h) => ({
        events: afterReset ? mergeTail(incoming, h.events) : page.reset ? incoming : unique([...incoming, ...h.events]),
        before: page.next_before, hasMore: page.has_more, loading: false, error: null,
      }));
    } catch (cause) {
      if (run !== generation || revision !== (revisions.get(agentId) ?? 0)) return;
      changeHistory(agentId, (h) => ({ ...h, loading: false, error: String(cause) }));
    }
  }

  function connect(sid: string, run: number) {
    if (source || run !== generation) return;
    const cursors = Object.fromEntries(Object.entries(get(state).histories).flatMap(([id, h]) => {
      const last = h.events.at(-1);
      return last ? [[id, last.cursor]] : [];
    }));
    const url = new URL(apiUrl(`/api/sessions/${encodeURIComponent(sid)}/agent-events/stream`));
    url.searchParams.set('cursors', checkpoint(cursors));
    const stream = new EventSource(url.toString());
    source = stream;
    const active = () => run === generation && source === stream;
    stream.onopen = () => { if (active()) state.update((s) => ({ ...s, connected: true, error: null })); };
    // Native EventSource retains Last-Event-ID and reconnects the same stream.
    stream.onerror = () => { if (active()) state.update((s) => ({ ...s, connected: false, error: 'Agent feed disconnected; reconnecting.' })); };
    stream.addEventListener('agent_event', (raw) => {
      if (!active()) return;
      try {
        const event: AgentEvent = JSON.parse((raw as MessageEvent<string>).data);
        if (!get(state).feeds[event.agent_id] || typeof event.cursor !== 'string') return;
        changeHistory(event.agent_id, (h) => h.events.some((item) => item.cursor === event.cursor) ? h
          : { ...h, events: [...h.events, event] });
      } catch {
        state.update((s) => ({ ...s, error: 'An agent event could not be read.' }));
      }
    });
    stream.addEventListener('feed_reset', (raw) => {
      if (!active()) return;
      try {
        const { agent_id } = JSON.parse((raw as MessageEvent<string>).data);
        if (!get(state).feeds[agent_id]) return;
        revisions.set(agent_id, (revisions.get(agent_id) ?? 0) + 1);
        changeHistory(agent_id, emptyHistory);
        void history(agent_id, false, true);
      } catch { state.update((s) => ({ ...s, error: 'An agent feed reset could not be read.' })); }
    });
    stream.addEventListener('feed_error', () => {
      if (!active()) return;
      stream.close();
      source = null;
      state.update((s) => ({ ...s, connected: false, error: 'Agent feed unavailable; reconnecting.' }));
      // Recreate from the last loaded per-agent cursors, never from a timestamp.
      retryTimer = setTimeout(() => connect(sid, run), 1000);
    });
  }

  async function refresh() {
    if (rosterLoading) return;
    const sid = get(state).sessionId;
    const controller = abort;
    const run = generation;
    if (!sid || !controller) return;
    rosterLoading = true;
    try {
      const roster = await read<{ session_id: string; agents: AgentFeed[] }>(`/api/sessions/${encodeURIComponent(sid)}/agent-feeds`, controller.signal);
      if (run !== generation) return;
      if (roster.session_id !== sid) throw new Error('Agent feed session mismatch');
      const previous = get(state).feeds;
      const feeds = Object.fromEntries(roster.agents.map((feed) => [feed.agent_id, feed]));
      for (const id of Object.keys(previous)) if (!feeds[id]) revisions.set(id, (revisions.get(id) ?? 0) + 1);
      state.update((s) => ({ ...s, feeds, error: s.connected ? null : s.error,
        histories: Object.fromEntries(Object.entries(s.histories).filter(([id]) => feeds[id])) }));
      await Promise.all(roster.agents.filter((feed) => feed.capabilities.event_source !== 'none'
        && (!previous[feed.agent_id] || previous[feed.agent_id].capabilities.event_source !== feed.capabilities.event_source
          || !get(state).histories[feed.agent_id] || get(state).histories[feed.agent_id].error))
        .map((feed) => history(feed.agent_id)));
      if (run === generation) connect(sid, run);
    } catch (cause) {
      if (run === generation) state.update((s) => ({ ...s, error: String(cause) }));
    } finally { if (run === generation) rosterLoading = false; }
  }

  function setSessionId(sessionId: string | null) {
    if (get(state).sessionId === sessionId) return;
    generation++;
    abort?.abort();
    source?.close();
    source = null;
    clearInterval(rosterTimer);
    clearTimeout(retryTimer);
    revisions.clear();
    rosterLoading = false;
    abort = sessionId ? new AbortController() : null;
    state.set(emptyState(sessionId));
    if (sessionId) {
      void refresh();
      rosterTimer = setInterval(() => void refresh(), 10_000);
    }
  }

  return { subscribe: state.subscribe, setSessionId, refresh, loadOlder: (agentId: string) => history(agentId, true), disconnect: () => setSessionId(null) };
}

export const agentEvents = createAgentEventsStore();
