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

function graphNode(id: string, folder: string) {
  return {
    id,
    title: id,
    folder,
    path: `${id}.md`,
    last_updated: '2026-07-19',
    in_degree: 0,
    out_degree: 0,
  };
}

describe('Knowledge route scope', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    knowledgeStore.reset();
    pageStore.set({ url: new URL('http://localhost/knowledge?session_id=session-a') });
    fetchMock = vi.fn().mockResolvedValue(jsonResponse({ nodes: [], edges: [] }));
    vi.stubGlobal('fetch', fetchMock);
    // The graph view mounts once nodes exist; jsdom has no ResizeObserver.
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    );
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

  it('offers every discovered folder in the filter, including names it has never seen', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse({
        nodes: [
          graphNode('patterns/known', 'patterns'),
          graphNode('agents/dossier', 'agents'),
          graphNode('zettelkasten/note', 'zettelkasten'),
          graphNode('root/index', 'root'),
          // A directory name as the operator actually typed it. Ranking must be
          // case-folded, or a capitalised curated name would silently sort to the
          // tail with no error.
          graphNode('Patterns Archive/old', 'Patterns Archive'),
          graphNode('Operations/runbook', 'Operations'),
        ],
        edges: [],
        omissions: [],
      }),
    );

    const { container } = render(KnowledgePage);
    await waitFor(() =>
      expect(container.querySelectorAll('.folder-field option').length).toBe(7),
    );

    const options = [...container.querySelectorAll('.folder-field option')].map(
      (option) => (option as HTMLOptionElement).value,
    );
    // Curated folders keep their preferred position regardless of casing;
    // unlisted ones follow, sorted, and are selectable rather than silently
    // missing from the filter.
    // The no-filter sentinel is `null`, which the DOM reflects as an empty value. It must NOT be
    // the string 'all': the folder set is discovered from the wiki root, so a real folder named
    // `all` would otherwise collide with the sentinel and be unselectable.
    expect(options[0]).toBe('');
    expect(options).not.toContain('all');
    expect(options).toEqual([
      '',
      'patterns',
      'Operations',
      'root',
      'agents',
      'Patterns Archive',
      'zettelkasten',
    ]);

    // Each folder's legend swatch is a real, distinct colour — the discovered
    // folders must not collapse onto one shared fallback.
    const swatches = [
      ...container.querySelectorAll('[aria-label="Folder colors and shapes"] i'),
    ].map((swatch) => (swatch as HTMLElement).style.background);
    expect(swatches.length).toBe(6);
    expect(swatches.every((background) => background.length > 0)).toBe(true);
    expect(new Set(swatches).size).toBe(6);
  });

  it('names what was omitted instead of showing one ambiguous cap banner', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse({
        nodes: [graphNode('patterns/known', 'patterns')],
        edges: [],
        truncated: true,
        omissions: [
          {
            reason: 'file_too_large',
            count: 3,
            detail: 'pages omitted: the file is larger than the read limit',
            examples: ['patterns/huge'],
          },
          {
            reason: 'edge_hint_too_long',
            count: 2,
            detail: 'link hints ignored: longer than the hint limit (all pages are shown)',
            examples: ['practices/workflow'],
          },
        ],
      }),
    );

    const { container } = render(KnowledgePage);
    await waitFor(() => expect(container.querySelector('.cap-notice')).not.toBeNull());

    const lines = [...container.querySelectorAll('.cap-notice li')].map(
      (line) => line.textContent?.trim() ?? '',
    );
    expect(lines).toEqual([
      '3 pages omitted: the file is larger than the read limit (patterns/huge, +2 more)',
      '2 link hints ignored: longer than the hint limit (all pages are shown) (practices/workflow, +1 more)',
    ]);
    // The old banner said only this, for any of a dozen causes.
    expect(container.querySelector('.cap-notice')?.textContent).not.toContain(
      'Refine the corpus',
    );
  });

  it('still warns when an older backend sends the bare boolean', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse({
        nodes: [graphNode('patterns/known', 'patterns')],
        edges: [],
        truncated: true,
      }),
    );

    const { container } = render(KnowledgePage);
    await waitFor(() => expect(container.querySelector('.cap-notice')).not.toBeNull());
    expect(container.querySelector('.cap-notice')?.textContent).toContain(
      'does not report which one',
    );
  });
});
