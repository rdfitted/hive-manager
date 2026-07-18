import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

function jsonResponse(payload: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: vi.fn().mockResolvedValue(payload),
  } as unknown as Response;
}

function graphPayload() {
  return {
    nodes: [
      {
        id: 'patterns/steady-state',
        title: 'Steady State',
        folder: 'patterns',
        path: 'patterns/steady-state.md',
        last_updated: '2026-07-18T00:00:00Z',
        in_degree: 1,
        out_degree: 1,
      },
      {
        id: 'practices/safe fetch',
        title: 'Safe Fetch',
        folder: 'practices',
        path: 'practices/safe-fetch.md',
        last_updated: null,
        in_degree: 1,
        out_degree: 0,
      },
    ],
    edges: [
      { source: 'patterns/steady-state', target: 'practices/safe fetch', kind: 'related' },
      { source: 'patterns/steady-state', target: 'clients/private', kind: 'cross_ref' },
    ],
    truncated: true,
  };
}

function graphWithout(id: string) {
  const graph = graphPayload();
  return {
    ...graph,
    nodes: graph.nodes.filter((node) => node.id !== id),
    edges: graph.edges.filter((edge) => edge.source !== id && edge.target !== id),
  };
}

describe('knowledge store', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.resetModules();
    fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('loads and normalizes the graph, then fetches a preview by encoded node id', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    fetchMock
      .mockResolvedValueOnce(jsonResponse(graphPayload()))
      .mockResolvedValueOnce(jsonResponse({
        id: 'practices/safe fetch',
        title: 'Safe Fetch',
        folder: 'practices',
        path: 'practices/safe-fetch.md',
        content: '# Safe fetch',
        last_updated: '2026-07-18T00:00:00Z',
        truncated: false,
      }));

    expect(await store.loadGraph()).toBe(true);
    const graphUrl = new URL(String(fetchMock.mock.calls[0][0]));
    expect(graphUrl.pathname).toBe('/api/knowledge/graph');
    expect(graphUrl.search).toBe('');
    expect(get(store).graph.nodes).toHaveLength(2);
    expect(get(store).graph.edges).toHaveLength(1);
    expect(get(store).graph.truncated).toBe(true);

    expect(await store.selectNode('practices/safe fetch')).toBe(true);
    const pageUrl = new URL(String(fetchMock.mock.calls[1][0]));
    expect(pageUrl.pathname).toBe('/api/knowledge/page');
    expect(pageUrl.search).toBe('?id=practices%2Fsafe%20fetch');
    expect(get(store).page?.title).toBe('Safe Fetch');
  });

  it('adds the exact encoded session id to graph, preview, and retry requests', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    const sessionId = 'session/alpha + beta&scope';
    const page = {
      id: 'practices/safe fetch',
      title: 'Safe Fetch',
      folder: 'practices',
      path: 'practices/safe-fetch.md',
      content: '# Safe fetch',
      last_updated: null,
    };
    fetchMock
      .mockResolvedValueOnce(jsonResponse(graphPayload()))
      .mockResolvedValueOnce(jsonResponse(page))
      .mockResolvedValueOnce(jsonResponse(page));

    expect(await store.loadGraph(sessionId)).toBe(true);
    expect(await store.selectNode('practices/safe fetch', sessionId)).toBe(true);
    expect(await store.retryPage(sessionId)).toBe(true);

    const graphUrl = new URL(String(fetchMock.mock.calls[0][0]));
    expect(graphUrl.pathname).toBe('/api/knowledge/graph');
    expect(graphUrl.search).toBe(
      '?session_id=session%2Falpha%20%2B%20beta%26scope',
    );

    for (const call of fetchMock.mock.calls.slice(1)) {
      const pageUrl = new URL(String(call[0]));
      expect(pageUrl.pathname).toBe('/api/knowledge/page');
      expect(pageUrl.search).toBe(
        '?id=practices%2Fsafe%20fetch&session_id=session%2Falpha%20%2B%20beta%26scope',
      );
    }
  });

  it('keeps the previous graph visible on refresh failure and exposes a friendly error', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    fetchMock
      .mockResolvedValueOnce(jsonResponse(graphPayload()))
      .mockResolvedValueOnce(jsonResponse({}, 503));

    await store.loadGraph();
    expect(await store.loadGraph()).toBe(false);

    expect(get(store).graph.nodes).toHaveLength(2);
    expect(get(store).loading).toBe(false);
    expect(get(store).refreshing).toBe(false);
    expect(get(store).error).toContain('(503)');
  });

  it('does not let an older preview response replace a newer selection', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    let resolveFirst!: (value: Response) => void;
    const first = new Promise<Response>((resolve) => { resolveFirst = resolve; });
    fetchMock
      .mockImplementationOnce(() => first)
      .mockResolvedValueOnce(jsonResponse({
        id: 'research/newer',
        title: 'Newer',
        folder: 'research',
        path: 'research/newer.md',
        content: 'newer',
        last_updated: null,
      }));

    const olderRequest = store.selectNode('patterns/older');
    const newerRequest = store.selectNode('research/newer');
    await newerRequest;
    resolveFirst(jsonResponse({
      id: 'patterns/older',
      title: 'Older',
      folder: 'patterns',
      path: 'patterns/older.md',
      content: 'older',
      last_updated: null,
    }));
    await olderRequest;

    expect(get(store).selectedId).toBe('research/newer');
    expect(get(store).page?.title).toBe('Newer');
  });

  it('clears a selection removed by refresh and ignores its in-flight preview', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    let resolvePage!: (value: Response) => void;
    const pageResponse = new Promise<Response>((resolve) => { resolvePage = resolve; });
    fetchMock
      .mockResolvedValueOnce(jsonResponse(graphPayload()))
      .mockImplementationOnce(() => pageResponse)
      .mockResolvedValueOnce(jsonResponse(graphWithout('patterns/steady-state')));

    await store.loadGraph();
    const pageRequest = store.selectNode('patterns/steady-state');
    expect(get(store).pageLoading).toBe(true);

    expect(await store.loadGraph()).toBe(true);
    expect(get(store)).toMatchObject({
      selectedId: null,
      page: null,
      pageLoading: false,
      pageError: null,
    });

    resolvePage(jsonResponse({
      id: 'patterns/steady-state',
      title: 'Steady State',
      folder: 'patterns',
      path: 'patterns/steady-state.md',
      content: 'stale preview',
      last_updated: null,
    }));
    expect(await pageRequest).toBe(false);
    expect(get(store)).toMatchObject({ selectedId: null, page: null, pageLoading: false });
  });

  it('clears a removed selection page error after a successful refresh', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    fetchMock
      .mockResolvedValueOnce(jsonResponse(graphPayload()))
      .mockResolvedValueOnce(jsonResponse({}, 503))
      .mockResolvedValueOnce(jsonResponse(graphWithout('patterns/steady-state')));

    await store.loadGraph();
    expect(await store.selectNode('patterns/steady-state')).toBe(false);
    expect(get(store).pageError).toContain('(503)');

    expect(await store.loadGraph()).toBe(true);
    expect(get(store)).toMatchObject({
      selectedId: null,
      page: null,
      pageLoading: false,
      pageError: null,
    });
  });

  it('preserves the selected page when its node remains after refresh', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    fetchMock
      .mockResolvedValueOnce(jsonResponse(graphPayload()))
      .mockResolvedValueOnce(jsonResponse({
        id: 'patterns/steady-state',
        title: 'Steady State',
        folder: 'patterns',
        path: 'patterns/steady-state.md',
        content: 'retained preview',
        last_updated: null,
      }))
      .mockResolvedValueOnce(jsonResponse(graphPayload()));

    await store.loadGraph();
    await store.selectNode('patterns/steady-state');
    const page = get(store).page;

    expect(await store.loadGraph()).toBe(true);
    expect(get(store)).toMatchObject({
      selectedId: 'patterns/steady-state',
      page,
      pageLoading: false,
      pageError: null,
    });
  });

  it('clears the same node preview when changing from session to global scope', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    const sessionId = 'session-a';
    fetchMock
      .mockResolvedValueOnce(jsonResponse(graphPayload()))
      .mockResolvedValueOnce(jsonResponse({
        id: 'patterns/steady-state',
        title: 'Steady State',
        folder: 'patterns',
        path: 'patterns/steady-state.md',
        content: 'session preview',
        last_updated: null,
      }))
      .mockResolvedValueOnce(jsonResponse(graphPayload()));

    await store.loadGraph(sessionId);
    await store.selectNode('patterns/steady-state', sessionId);
    expect(get(store).page?.content).toBe('session preview');

    expect(await store.loadGraph(null)).toBe(true);
    expect(get(store).graph.nodes.some((node) => node.id === 'patterns/steady-state')).toBe(true);
    expect(get(store)).toMatchObject({
      selectedId: null,
      page: null,
      pageLoading: false,
      pageError: null,
    });
  });

  it('invalidates an in-flight global preview when changing to session scope', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    let resolvePage!: (value: Response) => void;
    let resolveGraph!: (value: Response) => void;
    const pageResponse = new Promise<Response>((resolve) => { resolvePage = resolve; });
    const graphResponse = new Promise<Response>((resolve) => { resolveGraph = resolve; });
    fetchMock
      .mockResolvedValueOnce(jsonResponse(graphPayload()))
      .mockImplementationOnce(() => pageResponse)
      .mockImplementationOnce(() => graphResponse);

    await store.loadGraph(null);
    const pageRequest = store.selectNode('patterns/steady-state', null);
    expect(get(store).pageLoading).toBe(true);

    const scopedGraphRequest = store.loadGraph('session-b');
    expect(get(store)).toMatchObject({
      graph: { nodes: [], edges: [], truncated: false },
      loading: true,
      refreshing: false,
      selectedId: null,
      page: null,
      pageLoading: false,
      pageError: null,
    });

    resolvePage(jsonResponse({
      id: 'patterns/steady-state',
      title: 'Steady State',
      folder: 'patterns',
      path: 'patterns/steady-state.md',
      content: 'stale global preview',
      last_updated: null,
    }));
    expect(await pageRequest).toBe(false);
    expect(get(store)).toMatchObject({
      graph: { nodes: [], edges: [], truncated: false },
      loading: true,
      selectedId: null,
      page: null,
      pageLoading: false,
    });

    resolveGraph(jsonResponse(graphPayload()));
    expect(await scopedGraphRequest).toBe(true);
    expect(get(store).graph.nodes).toHaveLength(2);
  });

  it('keeps the previous scope cleared when a replacement scope load fails', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    let resolveGraph!: (value: Response) => void;
    const graphResponse = new Promise<Response>((resolve) => { resolveGraph = resolve; });
    fetchMock
      .mockResolvedValueOnce(jsonResponse(graphPayload()))
      .mockResolvedValueOnce(jsonResponse({
        id: 'patterns/steady-state',
        title: 'Steady State',
        folder: 'patterns',
        path: 'patterns/steady-state.md',
        content: 'scope a preview',
        last_updated: null,
      }))
      .mockImplementationOnce(() => graphResponse);

    await store.loadGraph('session-a');
    await store.selectNode('patterns/steady-state', 'session-a');
    expect(get(store).graph.nodes).toHaveLength(2);
    expect(get(store).page?.content).toBe('scope a preview');

    const replacementRequest = store.loadGraph('session-b');
    expect(get(store)).toMatchObject({
      graph: { nodes: [], edges: [], truncated: false },
      loading: true,
      refreshing: false,
      error: null,
      selectedId: null,
      page: null,
      pageLoading: false,
      pageError: null,
    });

    resolveGraph(jsonResponse({}, 503));
    expect(await replacementRequest).toBe(false);
    expect(get(store)).toMatchObject({
      graph: { nodes: [], edges: [], truncated: false },
      loading: false,
      refreshing: false,
      selectedId: null,
      page: null,
      pageLoading: false,
      pageError: null,
    });
    expect(get(store).error).toContain('(503)');
  });

  it('keeps only the latest scope visible when graph loads overlap', async () => {
    const { createKnowledgeStore } = await import('./knowledge');
    const store = createKnowledgeStore();
    let resolveMiddleGraph!: (value: Response) => void;
    const middleGraphResponse = new Promise<Response>((resolve) => {
      resolveMiddleGraph = resolve;
    });
    fetchMock
      .mockResolvedValueOnce(jsonResponse(graphPayload()))
      .mockImplementationOnce(() => middleGraphResponse)
      .mockResolvedValueOnce(jsonResponse(graphWithout('patterns/steady-state')));

    await store.loadGraph('session-a');
    const middleRequest = store.loadGraph('session-b');
    expect(get(store).graph.nodes).toHaveLength(0);

    expect(await store.loadGraph('session-c')).toBe(true);
    expect(get(store).graph.nodes.map((node) => node.id)).toEqual(['practices/safe fetch']);

    resolveMiddleGraph(jsonResponse(graphPayload()));
    expect(await middleRequest).toBe(false);
    expect(get(store).graph.nodes.map((node) => node.id)).toEqual(['practices/safe fetch']);
    expect(get(store).error).toBeNull();
  });
});
