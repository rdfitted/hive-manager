import { writable } from 'svelte/store';
import { apiUrl } from '$lib/config';

/**
 * A single row of SQLite-backed application state. `value` is parsed JSON (matches the
 * Rust `ApplicationStateRow` serde shape).
 */
export interface ApplicationStateRow {
  session_id: string;
  key: string;
  value: unknown;
  updated_at: number;
}

interface ApplicationStateData {
  /** Latest value per key for the active session. */
  rows: Record<string, ApplicationStateRow>;
  /** Exclusive watermark (max updated_at seen). */
  watermark: number;
  sessionId: string | null;
  loading: boolean;
  error: string | null;
}

/** Matches the backend `APPLICATION_STATE_POLL_INTERVAL` (1s). */
export const APPLICATION_STATE_POLL_INTERVAL = 1000;

function createApplicationStateStore() {
  const { subscribe, update, set } = writable<ApplicationStateData>({
    rows: {},
    watermark: 0,
    sessionId: null,
    loading: false,
    error: null,
  });

  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let activeSessionId: string | null = null;

  function mergeRows(incoming: ApplicationStateRow[]) {
    if (incoming.length === 0) return;
    update((state) => {
      const rows = { ...state.rows };
      let watermark = state.watermark;
      for (const row of incoming) {
        rows[row.key] = row;
        if (row.updated_at > watermark) watermark = row.updated_at;
      }
      return { ...state, rows, watermark };
    });
  }

  async function snapshot(sessionId: string): Promise<void> {
    const response = await fetch(apiUrl(`/api/sessions/${sessionId}/application-state`));
    if (!response.ok) throw new Error(`snapshot failed: ${response.status}`);
    const rows: ApplicationStateRow[] = await response.json();
    if (activeSessionId !== sessionId) return; // session switched during fetch
    const map: Record<string, ApplicationStateRow> = {};
    let watermark = 0;
    for (const row of rows) {
      map[row.key] = row;
      if (row.updated_at > watermark) watermark = row.updated_at;
    }
    update((state) => ({ ...state, rows: map, watermark, loading: false, error: null }));
  }

  async function pollOnce(sessionId: string): Promise<void> {
    let since = 0;
    update((state) => {
      since = state.watermark;
      return state;
    });
    const response = await fetch(
      apiUrl(`/api/sessions/${sessionId}/application-state/poll?since=${since}`)
    );
    if (!response.ok) throw new Error(`poll failed: ${response.status}`);
    const rows: ApplicationStateRow[] = await response.json();
    if (activeSessionId !== sessionId) return; // session switched during fetch
    mergeRows(rows);
  }

  return {
    subscribe,

    /** Begin snapshot-hydrate then watermark-poll for a session. Resets on switch. */
    async start(sessionId: string) {
      if (activeSessionId === sessionId && pollTimer) return;
      this.stop();
      activeSessionId = sessionId;
      set({ rows: {}, watermark: 0, sessionId, loading: true, error: null });

      try {
        await snapshot(sessionId);
      } catch (err) {
        if (activeSessionId === sessionId) {
          update((state) => ({ ...state, loading: false, error: String(err) }));
        }
      }

      pollTimer = setInterval(() => {
        if (activeSessionId !== sessionId) return;
        pollOnce(sessionId).catch((err) => {
          update((state) => ({ ...state, error: String(err) }));
        });
      }, APPLICATION_STATE_POLL_INTERVAL);
    },

    stop() {
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = null;
      }
      activeSessionId = null;
    },
  };
}

export const applicationState = createApplicationStateStore();
