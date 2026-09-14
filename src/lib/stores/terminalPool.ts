/**
 * Keep-alive pool for session terminal grids (#286).
 *
 * Switching sessions used to unmount every terminal pane and remount it empty. The pool
 * remembers the most recently visited sessions so `SessionOverview` can keep their grids
 * mounted (hidden) and reveal them intact, scrollback included, when the operator comes
 * back. It is bounded two ways: an LRU cap on the number of pooled sessions, and a grace
 * period after which a session in a terminal state is dropped so completed work does not
 * hold xterm buffers forever.
 */
import { writable } from 'svelte/store';

/** Most sessions whose grids stay mounted at once, the active one included. */
export const MAX_POOLED_SESSIONS = 4;

/** How long a finished session's grid stays pooled after the operator leaves it. */
export const TERMINAL_STATE_GRACE_MS = 5 * 60_000;

/** Session states after which no agent will write again. Shared with the grid. */
export const TERMINAL_SESSION_STATES: ReadonlySet<string> = new Set([
  'Completed',
  'Closed',
  'Closing',
  'Failed',
  'QaMaxRetriesExceeded',
]);

export interface PooledSession {
  sessionId: string;
  /** When this session last stopped being (or became) the active one. */
  lastActiveAt: number;
}

export interface SweepOptions {
  exists: (sessionId: string) => boolean;
  isTerminal: (sessionId: string) => boolean;
}

/**
 * Make `sessionId` the most recently used entry. The previous head is stamped with `now`
 * so its grace period starts when the operator left it. The result is capped at
 * `MAX_POOLED_SESSIONS`; the new head is never the one trimmed.
 */
export function touchPooled(
  entries: readonly PooledSession[],
  sessionId: string,
  now: number,
): PooledSession[] {
  const rest = entries
    .map((entry, index) => (index === 0 ? { ...entry, lastActiveAt: now } : entry))
    .filter((entry) => entry.sessionId !== sessionId);
  return [{ sessionId, lastActiveAt: now }, ...rest].slice(0, MAX_POOLED_SESSIONS);
}

/**
 * Drop entries whose session no longer exists, and finished sessions that have been
 * inactive longer than the grace period. The active session is always kept.
 */
export function sweepPooled(
  entries: readonly PooledSession[],
  activeSessionId: string | null,
  now: number,
  options: SweepOptions,
): PooledSession[] {
  return entries.filter((entry) => {
    if (entry.sessionId === activeSessionId) return true;
    if (!options.exists(entry.sessionId)) return false;
    if (!options.isTerminal(entry.sessionId)) return true;
    return now - entry.lastActiveAt < TERMINAL_STATE_GRACE_MS;
  });
}

function createTerminalPool() {
  const { subscribe, update, set } = writable<PooledSession[]>([]);

  return {
    subscribe,
    touch(sessionId: string, now: number = Date.now()) {
      update((entries) => touchPooled(entries, sessionId, now));
    },
    sweep(activeSessionId: string | null, options: SweepOptions, now: number = Date.now()) {
      update((entries) => sweepPooled(entries, activeSessionId, now, options));
    },
    remove(sessionId: string) {
      update((entries) => entries.filter((entry) => entry.sessionId !== sessionId));
    },
    reset() {
      set([]);
    },
  };
}

export const terminalPool = createTerminalPool();
