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
  const rosterCalls = () => fetcher.mock.calls.filter(([url]) => url.includes('agent-feeds')).length;

  it('polls the roster sooner only while a seat is due its first transcript', async () => {
    let pending = true;
    fetcher.mockImplementation(async (url: string) => url.includes('agent-feeds')
      ? ok({ session_id: 's', agents: [{ ...feed('a', 'none'), binding_pending: pending }] }) : ok(page([])));
    store.setSessionId('s'); await flush();
    expect(rosterCalls()).toBe(1);
    await vi.advanceTimersByTimeAsync(2_499);
    expect(rosterCalls()).toBe(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(rosterCalls(), 'an unbound seat is polled at 2.5s').toBe(2);
    pending = false;
    await vi.advanceTimersByTimeAsync(2_500);
    expect(rosterCalls()).toBe(3);
    await vi.advanceTimersByTimeAsync(9_999);
    expect(rosterCalls(), 'no extra polling once nothing is pending').toBe(3);
    await vi.advanceTimersByTimeAsync(1);
    expect(rosterCalls()).toBe(4);
    store.disconnect();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(rosterCalls(), 'a closed session stops polling').toBe(4);
  });

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

  it('keeps late-bound SSE history ordered when the roster loads an overlapping tail and reconnects from the newest event', async () => {
    let bound = false;
    const records = Array.from({ length: 250 }, (_, i) => event(`e${i + 1}`));
    fetcher.mockImplementation(async (url: string) => url.includes('agent-feeds')
      ? ok({ session_id: 's', agents: [feed('a', bound ? 'transcript' : 'none')] })
      : ok(page(records.slice(150), { next_before: 'e151', has_more: true })));
    store.setSessionId('s'); await flush();
    const stream = Stream.instances[0];
    const initialToken = new URL(stream.url).searchParams.get('cursors')!;
    expect(JSON.parse(atob(initialToken.replaceAll('-', '+').replaceAll('_', '/')))).toEqual({});
    expect(fetcher.mock.calls).toHaveLength(1);
    for (const record of records.slice(50)) stream.emit('agent_event', record);
    bound = true;
    await store.refresh();
    const history = get(store).histories.a.events;
    expect(history.map((item) => item.cursor), 'late-bound tail must preserve e51 through e250 in server order')
      .toEqual(records.slice(50).map((item) => item.cursor));
    expect(new Set(history.map((item) => item.cursor)).size, 'late-bound tail must not duplicate SSE records').toBe(200);
    stream.emit('feed_error', { error: 'fixture unavailable' });
    await vi.advanceTimersByTimeAsync(1000);
    const reconnectToken = new URL(Stream.instances[1].url).searchParams.get('cursors')!;
    expect(JSON.parse(atob(reconnectToken.replaceAll('-', '+').replaceAll('_', '/'))), 'late-bind reconnect checkpoint must be the newest event')
      .toEqual({ a: 'e250' });
  });

  it('F5: refetches a replacement generation longer than one page and retains paging, SSE order/dedup and revision guards', async () => {
    fetcher.mockReset().mockImplementation(async (url: string) => url.includes('agent-feeds')
      ? ok({ session_id: 's', agents: [feed('a')] })
      : ok(page([event('tail')], { has_more: true, next_before: 'tail' })));
    store.setSessionId('s'); await flush();
    let resolveOld!: (value: unknown) => void;
    let resolveTail!: (value: unknown) => void;
    fetcher.mockImplementationOnce(() => new Promise((done) => { resolveOld = done; }));
    const pending = store.loadOlder('a');
    fetcher.mockImplementationOnce(() => new Promise((done) => { resolveTail = done; }));
    const stream = Stream.instances[0];
    stream.emit('feed_reset', { agent_id: 'a' });
    expect(fetcher.mock.calls.at(-1)?.[0], 'feed_reset must refetch a tail page').not.toContain('before=');
    expect(get(store).histories.a.loading, 'feed_reset must refetch history metadata').toBe(true);
    const replacement = Array.from({ length: 250 }, (_, i) => event(`b-${i + 1}`));
    // SSE replays 200 records while HTTP returns a 100-record tail; one later record arrives too.
    for (const record of replacement.slice(50)) stream.emit('agent_event', record);
    stream.emit('agent_event', event('b-251'));
    resolveOld(ok(page([event('old-generation')]))); await pending;
    expect(get(store).histories.a.loading).toBe(true);
    resolveTail(ok(page(replacement.slice(150), { has_more: true, next_before: 'b-151' }))); await flush();
    expect(get(store).histories.a.hasMore, 'replacement tail must restore Load older').toBe(true);
    expect(get(store).histories.a.events.map((e) => e.cursor)).toEqual([...replacement.slice(50).map((e) => e.cursor), 'b-251']);
    fetcher.mockResolvedValueOnce(ok(page(replacement.slice(0, 150))));
    await store.loadOlder('a');
    expect(fetcher.mock.calls.at(-1)?.[0]).toContain('before=b-151');
    expect(get(store).histories.a.events.map((e) => e.cursor)).toEqual([...replacement.map((e) => e.cursor), 'b-251']);
    expect(get(store).histories.a.loading).toBe(false);
  });

  it('F5: a second reset or session change rejects an in-flight reset tail', async () => {
    store.setSessionId('s'); await flush();
    const stream = Stream.instances[0];
    let resolveFirst!: (value: unknown) => void;
    let resolveSecond!: (value: unknown) => void;
    fetcher.mockImplementationOnce(() => new Promise((done) => { resolveFirst = done; }));
    stream.emit('feed_reset', { agent_id: 'a' });
    fetcher.mockImplementationOnce(() => new Promise((done) => { resolveSecond = done; }));
    stream.emit('feed_reset', { agent_id: 'a' });
    stream.emit('agent_event', event('latest-generation'));
    resolveFirst(ok(page([event('superseded-generation')], { has_more: true, next_before: 'stale' }))); await flush();
    expect(get(store).histories.a.events.map((e) => e.cursor)).toEqual(['latest-generation']);
    expect(get(store).histories.a.loading).toBe(true);
    store.disconnect();
    resolveSecond(ok(page([event('late-tail')]))); await flush();
    expect(get(store).histories).toEqual({});
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
