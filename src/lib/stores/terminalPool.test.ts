import { describe, expect, it } from 'vitest';
import {
  MAX_POOLED_SESSIONS,
  TERMINAL_SESSION_STATES,
  TERMINAL_STATE_GRACE_MS,
  sweepPooled,
  touchPooled,
  type PooledSession,
} from './terminalPool';

function pooled(sessionId: string, lastActiveAt: number): PooledSession {
  return { sessionId, lastActiveAt };
}

describe('touchPooled', () => {
  it('moves the touched session to the front and stamps the session it replaced', () => {
    const entries = [pooled('a', 10), pooled('b', 5)];
    const next = touchPooled(entries, 'b', 100);

    expect(next.map((entry) => entry.sessionId)).toEqual(['b', 'a']);
    expect(next[0].lastActiveAt).toBe(100);
    expect(next[1].lastActiveAt).toBe(100);
  });

  it('caps the pool and never trims the session just touched', () => {
    let entries: PooledSession[] = [];
    for (let i = 0; i < 20; i++) entries = touchPooled(entries, `s${i}`, i);

    expect(entries).toHaveLength(MAX_POOLED_SESSIONS);
    expect(entries[0].sessionId).toBe('s19');
    expect(entries.map((entry) => entry.sessionId)).toEqual(['s19', 's18', 's17', 's16']);
  });

  it('keeps a bounded pool policy that leaves headroom for the renderer budget', () => {
    expect(MAX_POOLED_SESSIONS).toBe(4);
    expect(TERMINAL_STATE_GRACE_MS).toBe(5 * 60_000);
  });
});

describe('sweepPooled', () => {
  const options = (terminal: string[], missing: string[] = []) => ({
    exists: (id: string) => !missing.includes(id),
    isTerminal: (id: string) => terminal.includes(id),
  });

  it('keeps running sessions regardless of age', () => {
    const entries = [pooled('active', 0), pooled('old-running', 0)];
    expect(sweepPooled(entries, 'active', 10 * TERMINAL_STATE_GRACE_MS, options([]))).toEqual(entries);
  });

  it('drops a finished session only after a full grace period measured from the first terminal sweep', () => {
    const entries = [pooled('active', 0), pooled('done', 1000)];
    const seen = sweepPooled(entries, 'active', 2000, options(['done']));
    expect(seen.map((entry) => entry.sessionId)).toEqual(['active', 'done']);
    expect(seen[1].terminalSince).toBe(2000);

    const before = sweepPooled(seen, 'active', 2000 + TERMINAL_STATE_GRACE_MS - 1, options(['done']));
    const after = sweepPooled(seen, 'active', 2000 + TERMINAL_STATE_GRACE_MS, options(['done']));

    expect(before.map((entry) => entry.sessionId)).toEqual(['active', 'done']);
    expect(after.map((entry) => entry.sessionId)).toEqual(['active']);
  });

  it('starts the grace when the session finishes, not when the operator left it', () => {
    // Operator left a running session at t=0; it completes ten minutes later.
    let entries = [pooled('active', 0), pooled('late', 0)];
    const completedAt = 10 * TERMINAL_STATE_GRACE_MS;
    entries = sweepPooled(entries, 'active', completedAt - 1, options([]));
    expect(entries[1].terminalSince).toBeUndefined();

    entries = sweepPooled(entries, 'active', completedAt, options(['late']));
    expect(entries.map((entry) => entry.sessionId)).toEqual(['active', 'late']);
    expect(entries[1].terminalSince).toBe(completedAt);

    const kept = sweepPooled(entries, 'active', completedAt + TERMINAL_STATE_GRACE_MS - 1, options(['late']));
    const dropped = sweepPooled(entries, 'active', completedAt + TERMINAL_STATE_GRACE_MS, options(['late']));
    expect(kept.map((entry) => entry.sessionId)).toEqual(['active', 'late']);
    expect(dropped.map((entry) => entry.sessionId)).toEqual(['active']);
  });

  it('keeps the active session even when it is finished and stale, without running its clock', () => {
    const entries = [pooled('done-active', 0)];
    const swept = sweepPooled(entries, 'done-active', 10 * TERMINAL_STATE_GRACE_MS, options(['done-active']));
    expect(swept).toEqual(entries);
    expect(swept[0].terminalSince).toBeUndefined();
  });

  it('clears the clock if a finished session is resumed', () => {
    const entries = [{ ...pooled('resumed', 0), terminalSince: 500 }];
    const swept = sweepPooled(entries, 'active', 1000, options([]));
    expect(swept[0].terminalSince).toBeUndefined();
  });

  it('drops sessions that no longer exist', () => {
    const entries = [pooled('active', 0), pooled('deleted', 0)];
    expect(sweepPooled(entries, 'active', 0, options([], ['deleted'])).map((entry) => entry.sessionId)).toEqual(['active']);
  });

  it('shares the terminal-state vocabulary with the grid', () => {
    for (const state of ['Completed', 'Closed', 'Closing', 'Failed', 'QaMaxRetriesExceeded']) {
      expect(TERMINAL_SESSION_STATES.has(state)).toBe(true);
    }
    expect(TERMINAL_SESSION_STATES.has('Running')).toBe(false);
  });
});
