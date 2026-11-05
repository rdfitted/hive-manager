import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { Session } from '$lib/stores/sessions';
import type { CellStatus } from '$lib/types/domain';
import KanbanBoard from './KanbanBoard.svelte';

const mocks = vi.hoisted(() => ({
  set: (_items: Session[]) => {},
  classify: vi.fn<(state: unknown) => CellStatus>(),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('$lib/stores/sessions', async () => {
  const actual = await vi.importActual<typeof import('$lib/stores/sessions')>('$lib/stores/sessions');
  const { writable } = await import('svelte/store');
  const store = writable({ sessions: [] as Session[] });
  mocks.set = (items) => store.set({ sessions: items });
  mocks.classify.mockImplementation(actual.sessionStateToCellStatus);
  return { ...actual, sessions: { subscribe: store.subscribe, setActiveSession: vi.fn() },
    sessionStateToCellStatus: mocks.classify };
});

function session(name: string, state: unknown): Session {
  return { id: name, name, state: state as Session['state'], session_type: { Hive: { worker_count: 0 } },
    project_path: '/synthetic-project', created_at: '2026-11-04T12:00:00Z', agents: [] };
}

beforeEach(() => {
  mocks.set([]);
  mocks.classify.mockClear();
});
afterEach(cleanup);

describe('Kanban session display groups', () => {
  it('shows five primary columns and omits empty Other', () => {
    const view = render(KanbanBoard);
    expect(view.getAllByRole('region').map((column) => column.getAttribute('aria-label')))
      .toEqual(['Preparing', 'Running', 'Waiting Input', 'Completed', 'Failed']);
    expect(view.queryByRole('region', { name: 'Other' })).toBeNull();
  });

  it.each([
    { state: 'Starting', phase: 'launching', column: 'Running' },
    { state: 'Closing', phase: 'summarizing', column: 'Completed' },
  ])('folds $phase sessions into $column', ({ state, phase, column }) => {
    const item = session(`Synthetic ${phase}`, state);
    mocks.set([item]);
    const view = render(KanbanBoard);
    expect(within(view.getByRole('region', { name: column })).getByRole('article', { name: item.name })).toBeTruthy();
    expect(view.getAllByRole('article')).toHaveLength(1);
    expect(view.queryByRole('region', { name: 'Other' })).toBeNull();
  });

  it.each(['queued', 'killed', 'unknown'] as const)('keeps %s sessions in Other exactly once', (status) => {
    // Killed currently has no session-state producer; preserve the display's
    // total coverage if a classifier returns it in a future version.
    if (status === 'killed') mocks.classify.mockReturnValueOnce('killed');
    const item = session(`Synthetic ${status}`, status === 'unknown' ? 'FutureWorkflow' : 'Queued');
    mocks.set([item]);
    const view = render(KanbanBoard);
    expect(within(view.getByRole('region', { name: 'Other' })).getByRole('article', { name: item.name })).toBeTruthy();
    expect(view.getAllByRole('article')).toHaveLength(1);
  });

  it('places every session in exactly one of the primary groups or Other', () => {
    const states = ['Planning', 'Running', 'Paused', 'Completed', { Failed: 'Synthetic failure' }, 'Starting', 'Closing', 'FutureWorkflow'];
    const items = states.map((state, index) => session(`Synthetic session ${index}`, state));
    mocks.set(items);
    const view = render(KanbanBoard);
    expect(view.getAllByRole('article')).toHaveLength(items.length);
    for (const item of items) expect(view.getAllByRole('article', { name: item.name })).toHaveLength(1);
    for (const [name, count] of [['Preparing', 1], ['Running', 2], ['Waiting Input', 1], ['Completed', 2], ['Failed', 1], ['Other', 1]] as const) {
      expect(within(view.getByRole('region', { name })).getAllByRole('article')).toHaveLength(count);
    }
  });

  it('removes Other when its last session leaves', async () => {
    mocks.set([session('Synthetic future', 'FutureWorkflow')]);
    const view = render(KanbanBoard);
    expect(view.getByRole('region', { name: 'Other' })).toBeTruthy();
    mocks.set([session('Synthetic running', 'Running')]);
    await tick();
    expect(view.queryByRole('region', { name: 'Other' })).toBeNull();
    expect(view.getAllByRole('article')).toHaveLength(1);
  });
});
