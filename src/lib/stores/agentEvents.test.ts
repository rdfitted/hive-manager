import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { createAgentEventsStore, type AgentEvent, type AgentFeed } from './agentEvents';

class Stream {
  static instances: Stream[] = [];
  listeners = new Map<string, (raw: { data: string }) => void>();
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  constructor(public url: string) { Stream.instances.push(this); }
  addEventListener(type: string, listener: (raw: { data: string }) => void) { this.listeners.set(type, listener); }
  close() { this.closed = true; }
  emit(type: string, value: unknown) { this.listeners.get(type)?.({ data: JSON.stringify(value) }); }
}
const feed = (id: string, event_source: AgentFeed['capabilities']['event_source'] = 'transcript'): AgentFeed => ({
  agent_id: id, capabilities: { event_source, has_terminal: true, can_interrupt: true, supports_approvals: false },
});
const event = (cursor: string, agent_id = 'a'): AgentEvent => ({
  agent_id, cursor, kind: 'assistant_message', ts: '2026-01-01T00:00:00Z', source_id: cursor, truncated: false, payload: { text: 'scrubbed fixture' },
});
const page = (events: AgentEvent[], extra = {}) => ({ agent_id: 'a', events, next_before: null, latest_cursor: events.at(-1)?.cursor ?? null, has_more: false, reset: false, ...extra });
const ok = (body: unknown) => ({ ok: true, json: async () => body });
const flush = async () => { for (let i = 0; i < 15; i++) await Promise.resolve(); };
let store: ReturnType<typeof createAgentEventsStore>;
let fetcher: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.useFakeTimers();
  Stream.instances = [];
  vi.stubGlobal('EventSource', Stream);
  fetcher = vi.fn(async (url: string) => url.includes('agent-feeds')
    ? ok({ session_id: 's', agents: [feed('a'), feed('b', 'none')] }) : ok(page([event('tail')])));
  vi.stubGlobal('fetch', fetcher);
  store = createAgentEventsStore();
});
afterEach(() => { store.disconnect(); vi.useRealTimers(); vi.unstubAllGlobals(); });

describe('session agent event feed', () => {
  it('loads the tail before one multiplexed stream, without fetching unsupported feeds', async () => {
    store.setSessionId('s');
    store.setSessionId('s');
    expect(Stream.instances).toHaveLength(0);
    await flush();
    expect(fetcher.mock.calls.map(([url]) => new URL(url).pathname)).toEqual([
      '/api/sessions/s/agent-feeds', '/api/sessions/s/agents/a/events',
    ]);
    expect(Stream.instances).toHaveLength(1);
    const stream = Stream.instances[0];
    expect(new URL(stream.url).pathname).toBe('/api/sessions/s/agent-events/stream');
    const token = new URL(stream.url).searchParams.get('cursors')!;
    expect(JSON.parse(atob(token.replaceAll('-', '+').replaceAll('_', '/')))).toEqual({ a: 'tail' });
    expect(get(store).histories.a.events.map((e) => e.cursor)).toEqual(['tail']);
  });

  it('deduplicates replayed cursors on native reconnect and feed_error recovery', async () => {
    store.setSessionId('s'); await flush();
    const first = Stream.instances[0];
    first.onopen?.();
    first.emit('agent_event', event('tail'));
    first.emit('agent_event', event('live'));
    first.onerror?.();
    expect(get(store).connected).toBe(false);
    first.onopen?.();
    first.emit('agent_event', event('live'));
    expect(Stream.instances).toHaveLength(1);
    expect(get(store).histories.a.events.map((e) => e.cursor)).toEqual(['tail', 'live']);
    first.emit('feed_error', { error: 'fixture unavailable' });
    expect(first.closed).toBe(true);
    await vi.advanceTimersByTimeAsync(1000);
    expect(Stream.instances).toHaveLength(2);
    Stream.instances[1].emit('agent_event', event('live'));
    Stream.instances[1].emit('agent_event', event('after-reconnect'));
    expect(get(store).histories.a.events.map((e) => e.cursor)).toEqual(['tail', 'live', 'after-reconnect']);
  });

  it('prepends exclusive older pages, filters other-agent records and keeps live order', async () => {
    fetcher.mockImplementation(async (url: string) => url.includes('agent-feeds')
      ? ok({ session_id: 's', agents: [feed('a')] })
      : ok(url.includes('before=') ? page([event('old'), event('foreign', 'b')])
        : page([event('tail')], { next_before: 'tail', has_more: true })));
    store.setSessionId('s'); await flush();
    Stream.instances[0].emit('agent_event', event('live'));
    await store.loadOlder('a');
    expect(fetcher.mock.calls.at(-1)?.[0]).toContain('before=tail');
    expect(get(store).histories.a.events.map((e) => e.cursor)).toEqual(['old', 'tail', 'live']);
  });

  it('clears a reset generation and discards an in-flight page from the old generation', async () => {
    store.setSessionId('s'); await flush();
    let resolve!: (value: unknown) => void;
    fetcher.mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    // Seed a pageable history through a fresh roster source change.
    store.disconnect();
    fetcher.mockReset().mockImplementation(async (url: string) => url.includes('agent-feeds')
      ? ok({ session_id: 's', agents: [feed('a')] })
      : ok(page([event('tail')], { has_more: true, next_before: 'tail' })));
    store.setSessionId('s'); await flush();
    fetcher.mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const pending = store.loadOlder('a');
    Stream.instances.at(-1)!.emit('feed_reset', { agent_id: 'a' });
    Stream.instances.at(-1)!.emit('agent_event', event('new-generation'));
    resolve(ok(page([event('old-generation')]))); await pending;
    expect(get(store).histories.a.events.map((e) => e.cursor)).toEqual(['new-generation']);
    expect(get(store).histories.a.loading).toBe(false);
  });

  it('session changes close the old stream, clear data and reject late callbacks/responses', async () => {
    store.setSessionId('s'); await flush();
    const old = Stream.instances[0];
    let resolve!: (value: unknown) => void;
    fetcher.mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    store.setSessionId('other');
    expect(old.closed).toBe(true);
    expect(get(store).histories).toEqual({});
    old.emit('agent_event', event('late'));
    store.disconnect();
    resolve(ok({ session_id: 'other', agents: [feed('a')] })); await flush();
    expect(get(store).sessionId).toBeNull();
    expect(get(store).histories).toEqual({});
    expect(Stream.instances).toHaveLength(1);
  });

  it('refreshes newly added agents without opening extra streams and surfaces failures', async () => {
    store.setSessionId('s'); await flush();
    Stream.instances[0].onopen?.();
    fetcher.mockImplementation(async (url: string) => url.includes('agent-feeds')
      ? ok({ session_id: 's', agents: [feed('a'), feed('c')] })
      : ok(page([event('c-tail', 'c')], { agent_id: 'c' })));
    await store.refresh();
    expect(Stream.instances).toHaveLength(1);
    expect(get(store).histories.c.events[0].cursor).toBe('c-tail');
    fetcher.mockRejectedValueOnce(new Error('offline'));
    await store.refresh();
    expect(get(store).error).toContain('offline');
    await store.refresh();
    expect(get(store).error).toBeNull();
    expect(Stream.instances).toHaveLength(1);
  });
});
