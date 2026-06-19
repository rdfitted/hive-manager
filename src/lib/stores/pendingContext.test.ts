import { beforeEach, afterEach, describe, it, expect, vi } from 'vitest';
import type { PendingContext } from './pendingContext';

/**
 * pendingContext is backed by #124's application_state via the atomic take endpoint. We
 * mock fetch with a tiny in-memory KV that mirrors the backend `write` (POST
 * .../application-state) and `take` (POST .../application-state/take, read-and-delete)
 * semantics, so the one-turn-expiry guarantee is exercised against realistic responses.
 */

function makeBackend() {
  const kv = new Map<string, unknown>(); // key: `${sessionId}:${key}`

  const fetchMock = vi.fn(async (url: string | URL, init?: RequestInit) => {
    const u = typeof url === 'string' ? url : url.toString();
    const body = init?.body ? JSON.parse(init.body as string) : {};
    const sessionMatch = u.match(/\/api\/sessions\/([^/]+)\/application-state(\/take)?$/);
    const sessionId = sessionMatch?.[1] ?? '';
    const isTake = !!sessionMatch?.[2];

    if (isTake) {
      const k = `${sessionId}:${body.key}`;
      const value = kv.has(k) ? kv.get(k) : null;
      kv.delete(k); // atomic read-and-delete
      const row = value == null ? null : { session_id: sessionId, key: body.key, value, updated_at: 1 };
      return new Response(JSON.stringify(row), { status: 200 });
    }
    // write (capture)
    const k = `${sessionId}:${body.key}`;
    kv.set(k, body.value);
    return new Response(
      JSON.stringify({ session_id: sessionId, key: body.key, value: body.value, updated_at: 1 }),
      { status: 200 }
    );
  });

  return { kv, fetchMock };
}

describe('pendingContext store', () => {
  let restoreFetch: typeof globalThis.fetch;

  beforeEach(() => {
    vi.resetModules();
    restoreFetch = globalThis.fetch;
  });

  afterEach(() => {
    globalThis.fetch = restoreFetch;
    vi.restoreAllMocks();
  });

  const ctx = (over: Partial<PendingContext> = {}): PendingContext => ({
    sessionId: 's1',
    agentId: null,
    kind: 'selection',
    text: 'selected code',
    capturedAt: 123,
    ...over,
  });

  it('capture then consume returns the context once, then null (one-turn expiry)', async () => {
    const { fetchMock } = makeBackend();
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;
    const { pendingContext } = await import('./pendingContext');

    await pendingContext.capture(ctx());

    const first = await pendingContext.consume('s1');
    expect(first).not.toBeNull();
    expect(first?.text).toBe('selected code');

    // Second consume: the row was deleted by the atomic take, so null.
    const second = await pendingContext.consume('s1');
    expect(second).toBeNull();
  });

  it('capture twice keeps only the latest context', async () => {
    const { fetchMock } = makeBackend();
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;
    const { pendingContext } = await import('./pendingContext');

    await pendingContext.capture(ctx({ text: 'first' }));
    await pendingContext.capture(ctx({ text: 'second' }));

    const taken = await pendingContext.consume('s1');
    expect(taken?.text).toBe('second');
  });

  it('render() produces a fenced [Operator context] block', async () => {
    const { fetchMock } = makeBackend();
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;
    const { pendingContext } = await import('./pendingContext');

    const block = pendingContext.render(ctx({ text: 'abc' }));
    expect(block).toBe('[Operator context]\nabc\n[/Operator context]');

    const cellBlock = pendingContext.render(ctx({ kind: 'cell', cellId: 'cell-9', text: null }));
    expect(cellBlock).toContain('Selected session cell: cell-9');
  });

  it('falls back to the in-memory mirror if the backend take fails', async () => {
    const failing = vi.fn(async (url: string | URL, init?: RequestInit) => {
      const u = typeof url === 'string' ? url : url.toString();
      if (u.endsWith('/take')) {
        return new Response('err', { status: 500 });
      }
      return new Response('{}', { status: 200 }); // capture succeeds
    });
    globalThis.fetch = failing as unknown as typeof globalThis.fetch;
    const { pendingContext } = await import('./pendingContext');

    await pendingContext.capture(ctx({ text: 'mirror me' }));
    const taken = await pendingContext.consume('s1');
    expect(taken?.text).toBe('mirror me');

    // And it clears after the fallback consume.
    const again = await pendingContext.consume('s1');
    expect(again).toBeNull();
  });
});
