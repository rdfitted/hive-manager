import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';

const storeMocks = vi.hoisted(() => ({
  setActiveSession: undefined as ((session: { id: string } | null) => void) | undefined,
}));

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  const activeSession = writable<{ id: string } | null>({ id: 'session-a' });
  storeMocks.setActiveSession = activeSession.set;
  return { activeSession };
});

vi.mock('$lib/config', () => ({
  apiUrl: (path: string) => `http://localhost:18800${path}`,
}));

import WorkGraphView from './WorkGraphView.svelte';

const ROLE = (value: string) => ({ kind: 'role' as const, value });

function payload(overrides: Record<string, unknown> = {}) {
  return {
    view: 'runtime',
    source: 'live',
    nodes: [
      { id: 'T1', status: 'completed', lane: ROLE('backend'), contract_summary: { input_count: 1, output_count: 1, acceptance_count: 1 } },
      { id: 'T2', status: 'blocked', lane: ROLE('backend'), contract_summary: { input_count: 1, output_count: 1, acceptance_count: 1 } },
      { id: 'T3', status: 'pending', lane: ROLE('frontend'), contract_summary: { input_count: 0, output_count: 1, acceptance_count: 1 } },
    ],
    edges: [
      { source: 'T1', target: 'T2', kind: 'depends_on', provenance: 'planner' },
      { source: 'T1', target: 'T3', kind: 'touches', provenance: 'codegraph' },
    ],
    waves: [['T1'], ['T2', 'T3']],
    status_by_node: { T1: 'completed', T2: 'blocked', T3: 'pending' },
    lane_assignment: { T1: ROLE('backend'), T2: ROLE('backend'), T3: ROLE('frontend') },
    critical_path: ['T1', 'T2'],
    provenance_by_edge: [],
    divergence: null,
    ...overrides,
  };
}

function mockFetch(body: unknown, ok = true, status = 200) {
  const fetchMock = vi.fn().mockResolvedValue({
    ok,
    status,
    json: async () => body,
    text: async () => JSON.stringify(body),
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

/** Let the $effect fire and its awaited fetch settle. */
async function settle() {
  await tick();
  await Promise.resolve();
  await tick();
}

beforeEach(() => {
  vi.useFakeTimers();
  storeMocks.setActiveSession?.({ id: 'session-a' });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('WorkGraphView', () => {
  it('requests the work-graph projection for the active session', async () => {
    const fetchMock = mockFetch(payload());
    render(WorkGraphView);
    await settle();

    expect(fetchMock).toHaveBeenCalledWith(
      'http://localhost:18800/api/sessions/session-a/work-graph?view=runtime'
    );
  });

  it('renders one node per task, positioned by wave', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    const boxes = container.querySelectorAll('.wg-box');
    expect(boxes).toHaveLength(3);

    // Wave 0 sits above wave 1 — the layout must express dependency depth.
    // Match on the rendered label — a node's textContent also carries its
    // <title> tooltip, so a whole-node text match never equals the bare id.
    const y = (id: string) =>
      Number(
        [...container.querySelectorAll('.wg-node')]
          .find((node) => node.querySelector('.wg-label')?.textContent?.trim() === id)
          ?.querySelector('.wg-box')
          ?.getAttribute('y')
      );
    expect(y('T1')).toBeLessThan(y('T2'));
    expect(y('T2')).toBe(y('T3'));
  });

  it('renders blocked and not-started nodes with different classes', async () => {
    // The acceptance criterion: these two read identically in a task list and
    // must never read identically here.
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    expect(container.querySelector('.wg-box--blocked')).not.toBeNull();
    expect(container.querySelector('.wg-box--pending')).not.toBeNull();
    expect(container.querySelector('.wg-box--blocked')?.className.baseVal).not.toBe(
      container.querySelector('.wg-box--pending')?.className.baseVal
    );
  });

  it('marks critical-path nodes distinctly', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    expect(container.querySelectorAll('.wg-node.critical')).toHaveLength(2);
  });

  it('distinguishes edge provenance classes', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    expect(container.querySelector('.wg-edge--planner')).not.toBeNull();
    expect(container.querySelector('.wg-edge--codegraph')).not.toBeNull();
  });

  it('explains an empty graph instead of rendering a blank canvas', async () => {
    mockFetch(
      payload({ nodes: [], edges: [], waves: [], status_by_node: {}, lane_assignment: {}, critical_path: [] })
    );
    const { container } = render(WorkGraphView);
    await settle();

    expect(screen.getByText('No work graph for this session')).toBeTruthy();
    expect(container.querySelector('.wg-svg')).toBeNull();
  });

  it('surfaces a failed request rather than an empty state', async () => {
    mockFetch({ error: 'Session not found: session-a' }, false, 404);
    render(WorkGraphView);
    await settle();

    expect(screen.getByText('Could not load the work graph')).toBeTruthy();
    expect(screen.getByText('Session not found: session-a')).toBeTruthy();
  });
});
