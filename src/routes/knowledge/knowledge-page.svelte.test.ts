import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, waitFor } from '@testing-library/svelte';

const pageStore = vi.hoisted(() => {
  type PageValue = { url: URL };
  let value: PageValue = { url: new URL('http://localhost/knowledge') };
  const subscribers = new Set<(next: PageValue) => void>();
  return {
    subscribe(run: (next: PageValue) => void) {
      subscribers.add(run);
      run(value);
      return () => subscribers.delete(run);
    },
    set(next: PageValue) {
      value = next;
      for (const subscriber of subscribers) subscriber(value);
    },
  };
});

vi.mock('$lib/knowledge/navigation', () => ({ routePage: pageStore }));

import KnowledgePage from './+page.svelte';
import { knowledgeStore } from '$lib/stores/knowledge';

function jsonResponse(payload: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: vi.fn().mockResolvedValue(payload),
  } as unknown as Response;
}

describe('Knowledge route scope', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    knowledgeStore.reset();
    pageStore.set({ url: new URL('http://localhost/knowledge?session_id=session-a') });
    fetchMock = vi.fn().mockResolvedValue(jsonResponse({ nodes: [], edges: [] }));
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    cleanup();
    knowledgeStore.reset();
    vi.unstubAllGlobals();
  });

  it('reloads the graph when query navigation changes the session scope', async () => {
    render(KnowledgePage);
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

    const firstUrl = new URL(String(fetchMock.mock.calls[0][0]));
    expect(firstUrl.pathname).toBe('/api/knowledge/graph');
    expect(firstUrl.searchParams.get('session_id')).toBe('session-a');

    pageStore.set({ url: new URL('http://localhost/knowledge?session_id=session-b') });
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));

    const secondUrl = new URL(String(fetchMock.mock.calls[1][0]));
    expect(secondUrl.pathname).toBe('/api/knowledge/graph');
    expect(secondUrl.searchParams.get('session_id')).toBe('session-b');
  });
});
