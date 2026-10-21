import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import type { AgentInfo } from './sessions';

const mocks = vi.hoisted(() => ({ loadHeartbeats: vi.fn().mockResolvedValue(undefined) }));
vi.mock('./sessions', async () => ({ activeAgents: (await import('svelte/store')).writable([]), activeSession: (await import('svelte/store')).writable(null) }));
vi.mock('./conversations', async () => ({ heartbeatStore: { ...(await import('svelte/store')).writable({ agents: {}, staleAgents: new Set() }), loadHeartbeats: mocks.loadHeartbeats } }));
vi.mock('./events', async () => ({ events: (await import('svelte/store')).writable({ events: [] }) }));
import { activeAgents, activeSession } from './sessions';
import { heartbeatStore } from './conversations';
import { events } from './events';
import { agentActivity, attentionReason, needsAttention, now } from './agentActivity';

const agent = (status: AgentInfo['status']): AgentInfo => ({ id: 'a', role: 'Queen', status, config: { cli: 'codex', flags: [] }, parent_id: null });
const set = (store: unknown, value: unknown) => (store as { set(value: unknown): void }).set(value);
beforeEach(() => {
  mocks.loadHeartbeats.mockReset().mockResolvedValue(undefined);
  set(activeSession, null);
  set(activeAgents, [agent('Running')]);
  set(heartbeatStore, { agents: {}, staleAgents: new Set() });
  set(events, { events: [] });
});
afterEach(() => vi.useRealTimers());

describe('G-attention-predicate', () => {
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
  it('loads summary data without a Chat component and stops polling when no session is selected', async () => {
    vi.useFakeTimers();
    mocks.loadHeartbeats.mockImplementation(async () => {
      set(heartbeatStore, { agents: { a: { summary: 'Loaded outside Chat', timestamp: '2026-10-21T12:00:00Z' } }, staleAgents: new Set() });
    });
    set(activeSession, { id: 'one' });
    const values: string[] = [];
    const stop = agentActivity.subscribe(value => values.push(value.a.summary));
    try {
      await Promise.resolve();
      expect(values.at(-1)).toBe('Loaded outside Chat');
      set(activeSession, null);
      expect(vi.getTimerCount()).toBe(0);
    } finally { stop(); }
  });
});
