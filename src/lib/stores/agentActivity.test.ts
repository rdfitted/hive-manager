import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import type { AgentInfo } from './sessions';

const mocks = vi.hoisted(() => ({ loadHeartbeats: vi.fn().mockResolvedValue(undefined) }));
vi.mock('./sessions', async () => ({
  activeAgents: (await import('svelte/store')).writable([]),
  activeSession: (await import('svelte/store')).writable(null),
  sessions: (await import('svelte/store')).writable({ sessions: [] }),
  serdeEnumVariantName: (value: unknown) => typeof value === 'string' ? value : value && typeof value === 'object' ? Object.keys(value)[0] : undefined,
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('./conversations', async () => ({ heartbeatStore: { ...(await import('svelte/store')).writable({ agents: {}, staleAgents: new Set() }), loadHeartbeats: mocks.loadHeartbeats } }));
vi.mock('./events', async () => ({ events: (await import('svelte/store')).writable({ events: [] }) }));
import { activeAgents, activeSession, sessions } from './sessions';
import { heartbeatStore } from './conversations';
import { events } from './events';
import { agentActivity, attentionReason, needsAttention, now, staleRunningAgents } from './agentActivity';

const agent = (status: AgentInfo['status']): AgentInfo => ({ id: 'a', role: 'Queen', status, config: { cli: 'codex', flags: [] }, parent_id: null });
const set = (store: unknown, value: unknown) => (store as { set(value: unknown): void }).set(value);
beforeEach(() => {
  mocks.loadHeartbeats.mockReset().mockResolvedValue(undefined);
  set(activeSession, null);
  set(sessions, { sessions: [] });
  set(activeAgents, [agent('Running')]);
  set(heartbeatStore, { agents: {}, staleAgents: new Set() });
  set(events, { events: [] });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe('G-attention-predicate', () => {
  it('manually stopped agents never alarm even with an old heartbeat', () => {
    const stopped = agent({ WaitingForInput: 'stopped_by_operator' });
    expect(attentionReason(stopped, new Set(['a']))).toBe('none');
    expect(needsAttention(stopped, new Set(['a']))).toBe(false);
  });
  it.each(['finished', 'retired'])('G31: %s work has no stale attention or needs-you count', (work_state) => {
    const finished = { ...agent('Running'), work_state };
    const stale = new Set(['a']);
    expect(attentionReason(finished, stale)).toBe('none');
    expect([finished].filter(seat => needsAttention(seat, stale))).toHaveLength(0);
    expect(attentionReason(finished, stale, 'working')).toBe('stale');
  });
  it.each([undefined, 'unknown', 'working'])('G31: %s work state keeps the stale rule', (work_state) => {
    expect(attentionReason({ ...agent('Running'), work_state }, new Set(['a']))).toBe('stale');
  });
  it.each([{ Error: 'failed' }, { Error: 'exited_during_startup:101' },
    { WaitingForInput: 'injected_delivery_failed:test' }] as const)('G31: %o still alarms after finished proof', (status) => {
    expect(needsAttention({ ...agent(status), work_state: 'finished' }, new Set())).toBe(true);
  });
  it.each(['awaiting_injected_turn', 'awaiting_injected_turn:2026-10-21', 'other reason', 'injected_delivery_failed'])('does not flag WaitingForInput %s', (reason) => {
    expect(needsAttention(agent({ WaitingForInput: reason }), new Set())).toBe(false);
    expect(attentionReason(agent({ WaitingForInput: reason }), new Set())).toBe('none');
  });
  it.each(['injected_delivery_failed:transcript:123', 'injected_delivery_failed:'])('flags failed delivery prefix %s', (reason) => {
    expect(needsAttention(agent({ WaitingForInput: reason }), new Set())).toBe(true);
    expect(attentionReason(agent({ WaitingForInput: reason }), new Set())).toBe('delivery_failed');
  });
  it('flags Error and stale, including independently stale waiting agents', () => {
    expect(attentionReason(agent({ Error: 'crashed' }), new Set())).toBe('error');
    expect(needsAttention(agent({ Error: 'crashed' }), new Set())).toBe(true);
    expect(attentionReason(agent('Running'), new Set(['a']))).toBe('stale');
    expect(needsAttention(agent({ WaitingForInput: 'awaiting_injected_turn:1' }), new Set(['a']))).toBe(true);
    expect(needsAttention(agent('Running'), new Set(['other']))).toBe(false);
  });
});

describe('activity derivation', () => {
  it('G59 stale: Running crosses the boundary at 3m00s, with activity and finished-work exclusions', () => {
    const running = agent('Running');
    const at = Date.parse('2030-01-01T12:00:00Z');
    expect(staleRunningAgents([running], { a: at }, at + 179_000, at).size).toBe(0);
    expect([...staleRunningAgents([running], { a: at }, at + 180_000, at)]).toEqual(['a']);
    expect(staleRunningAgents([running], { a: at + 1_000 }, at + 180_000, at).size).toBe(0);
    expect([...staleRunningAgents([running], {}, at + 180_000, at)]).toEqual(['a']);
    for (const other of [{ ...running, status: 'Completed' as const }, { ...running, status: { WaitingForInput: 'prompt' } }, { ...running, work_state: 'finished' }, { ...running, work_state: 'retired' }]) {
      expect(staleRunningAgents([other], {}, at + 180_000, at).size).toBe(0);
    }
  });
  it('G31 confirmed follow-up exposes the input time after the awaiting marker is gone', () => {
    const latest_input_at = '2026-11-01T12:00:00Z';
    set(heartbeatStore, { agents: { a: { summary: 'Previous completion',
      timestamp: '2026-11-01T11:50:00Z', latest_input_at, work_state: 'working' } }, staleAgents: new Set() });
    expect(get(agentActivity).a.lastActivityAt).toBe(Date.parse(latest_input_at));
    expect(get(agentActivity).a.attention).toBe('none');
    set(heartbeatStore, { agents: { a: { timestamp: '2026-11-01T12:01:00Z',
      latest_input_at, work_state: 'working' } }, staleAgents: new Set(['a']) });
    expect(get(agentActivity).a.lastActivityAt).toBe(Date.parse('2026-11-01T12:01:00Z'));
    expect(get(agentActivity).a.attention).toBe('stale');
  });
  it('G31: fresh backend work state overrides the older roster state', () => {
    set(activeAgents, [{ ...agent('Running'), work_state: 'finished' }]);
    set(heartbeatStore, { agents: { a: { work_state: 'working' } }, staleAgents: new Set(['a']) });
    expect(get(agentActivity).a.attention).toBe('stale');
    set(heartbeatStore, { agents: { a: { work_state: 'finished' } }, staleAgents: new Set(['a']) });
    expect(get(agentActivity).a.attention).toBe('none');
    set(heartbeatStore, { agents: { a: { status: 'completed' } }, staleAgents: new Set(['a']) });
    expect(get(agentActivity).a.attention, 'missing API work state must not inherit old finished roster proof').toBe('stale');
  });
  it('normalizes summary and combines newest heartbeat/event time without a request', () => {
    set(heartbeatStore, { agents: { a: { summary: ' Fixing\n  tests ', timestamp: '2026-10-21T12:00:00Z' } }, staleAgents: new Set(['a']) });
    set(events, { events: [
      { agent_id: 'a', timestamp: '2026-10-21T12:01:00Z', event_type: 'agent_completed' },
      { agent_id: 'a', timestamp: '2026-10-21T11:59:00Z', event_type: 'agent_launched' },
      { agent_id: 'other', timestamp: '2026-10-21T12:02:00Z', event_type: 'agent_failed' },
    ] });
    expect(get(agentActivity).a).toEqual({ agentId: 'a', summary: 'Fixing tests', lastActivityAt: Date.parse('2026-10-21T12:01:00Z'), latestEventLabel: 'agent completed', attention: 'stale' });
  });
  it('keeps missing/invalid heartbeats unknown and removes vanished agents', () => {
    expect(get(agentActivity).a.lastActivityAt).toBeNull();
    set(heartbeatStore, { agents: { a: { summary: '', timestamp: 'invalid' } }, staleAgents: new Set() });
    expect(get(agentActivity).a.lastActivityAt).toBeNull();
    set(activeAgents, []);
    expect(get(agentActivity)).toEqual({});
  });
  it('shares one 30 second timer and releases it after the last subscriber', () => {
    vi.useFakeTimers();
    const one = vi.fn(); const two = vi.fn();
    const stopOne = now.subscribe(one); const stopTwo = now.subscribe(two);
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(30_000);
    expect(one).toHaveBeenCalledTimes(2); expect(two).toHaveBeenCalledTimes(2);
    stopOne(); expect(vi.getTimerCount()).toBe(1);
    stopTwo(); expect(vi.getTimerCount()).toBe(0);
  });
});


describe('shared heartbeat refresh without Chat mounted', () => {
  it('starts once, uses the existing loader every 10s, re-keys, and stops at the last subscriber', async () => {
    vi.useFakeTimers();
    set(sessions, { sessions: [{ id: 'one', state: 'Running' }] });
    set(activeSession, { id: 'one' });
    const stopOne = agentActivity.subscribe(() => {});
    const stopTwo = agentActivity.subscribe(() => {});
    try {
      expect(mocks.loadHeartbeats).toHaveBeenCalledTimes(1);
      expect(mocks.loadHeartbeats).toHaveBeenLastCalledWith('one');
      expect(vi.getTimerCount()).toBe(1);
      await vi.advanceTimersByTimeAsync(10_000);
      expect(mocks.loadHeartbeats).toHaveBeenCalledTimes(2);
      set(activeSession, { id: 'one', state: 'Running' });
      expect(mocks.loadHeartbeats).toHaveBeenCalledTimes(2);
      set(activeSession, { id: 'two' });
      expect(mocks.loadHeartbeats).toHaveBeenLastCalledWith('two');
      stopOne(); expect(vi.getTimerCount()).toBe(1);
      stopTwo(); expect(vi.getTimerCount()).toBe(0);
      const calls = mocks.loadHeartbeats.mock.calls.length;
      set(activeSession, { id: 'three' });
      await vi.advanceTimersByTimeAsync(30_000);
      expect(mocks.loadHeartbeats).toHaveBeenCalledTimes(calls);
    } finally { stopOne(); stopTwo(); }
  });
  it('loads summary data without a Chat component and stops polling after the last active session finishes', async () => {
    vi.useFakeTimers();
    mocks.loadHeartbeats.mockImplementation(async () => {
      set(heartbeatStore, { agents: { a: { summary: 'Loaded outside Chat', timestamp: '2026-10-21T12:00:00Z' } }, staleAgents: new Set() });
    });
    set(sessions, { sessions: [{ id: 'one', state: 'Running' }] });
    set(activeSession, { id: 'one' });
    const values: string[] = [];
    const stop = agentActivity.subscribe(value => values.push(value.a.summary));
    try {
      await Promise.resolve();
      expect(values.at(-1)).toBe('Loaded outside Chat');
      set(activeSession, null);
      set(sessions, { sessions: [{ id: 'one', state: 'Completed' }] });
      expect(vi.getTimerCount()).toBe(0);
    } finally { stop(); }
  });
});


describe('G52 no-selection sidebar polling', () => {
  it('fetches once per tick for all rows and stops for a finished-only list', async () => {
    vi.useFakeTimers();
    const actual = await vi.importActual<typeof import('./conversations')>('./conversations');
    const old = new Date(Date.now() - 4 * 60_000).toISOString();
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ sessions: [
      { id: 'one', agents: [{ id: 'stale-one', last_activity: old, work_state: 'working' }] },
      { id: 'two', agents: [{ id: 'stale-two', last_activity: old, work_state: 'working' }] },
    ] }) });
    vi.stubGlobal('fetch', fetchMock);
    mocks.loadHeartbeats.mockImplementation((id) => actual.heartbeatStore.loadHeartbeats(id));
    set(sessions, { sessions: [{ id: 'one', state: 'Running' }, { id: 'two', state: 'Paused' }] });
    const stopOne = agentActivity.subscribe(() => {});
    const stopTwo = agentActivity.subscribe(() => {});
    try {
      await vi.advanceTimersByTimeAsync(0);
      expect(fetchMock).toHaveBeenCalledTimes(1);
      expect(mocks.loadHeartbeats).toHaveBeenLastCalledWith(null);
      expect(get(actual.heartbeatStore).agents).toEqual({});
      expect([...get(actual.heartbeatStore).staleAgentsBySession.one]).toEqual(['stale-one']);
      expect([...get(actual.heartbeatStore).staleAgentsBySession.two]).toEqual(['stale-two']);
      await vi.advanceTimersByTimeAsync(10_000);
      expect(fetchMock).toHaveBeenCalledTimes(2);
      stopOne();
      await vi.advanceTimersByTimeAsync(10_000);
      expect(fetchMock).toHaveBeenCalledTimes(3);
      set(sessions, { sessions: [{ id: 'one', state: 'Completed' }, { id: 'two', state: { Failed: 'synthetic' } },
        { id: 'three', state: 'Closed' }, { id: 'four', state: 'Closing' }] });
      expect(vi.getTimerCount()).toBe(0);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(fetchMock).toHaveBeenCalledTimes(3);
    } finally { stopOne(); stopTwo(); }
  });
});
