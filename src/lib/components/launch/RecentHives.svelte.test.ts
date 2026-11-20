import { cleanup, fireEvent, render, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

import type { AgentInfo, Session } from '$lib/stores/sessions';
import RecentHives from './RecentHives.svelte';

afterEach(cleanup);

function agent(id: string, status: AgentInfo['status']): AgentInfo {
  return { id, role: id === 'queen' ? 'Queen' : { Worker: { index: 1, parent: null } }, status, parent_id: null,
    config: { cli: 'codex', flags: [], label: id } };
}

function session(id: string, name: string, at: string, agents: AgentInfo[], state: Session['state'] = 'Running'): Session {
  return { id, name, state, session_type: { Hive: { worker_count: agents.length - 1 } }, project_path: `D:/${id}`,
    created_at: '2026-01-01T00:00:00Z', last_activity_at: at, agents };
}

const hives = [
  session('old', 'Older hive', '2026-01-02T00:00:00Z', [agent('queen', 'Completed')], 'Completed'),
  session('new', 'Newest hive', '2026-01-05T00:00:00Z', [agent('queen', 'Running'), agent('worker', { Error: 'boom' })]),
  session('mid', 'Middle hive', '2026-01-03T00:00:00Z', [agent('queen', 'Running')]),
  session('first', 'First hive', '2026-01-01T00:00:00Z', [agent('queen', 'Running')]),
];

describe('RecentHives', () => {
  it('lists the most recently active hives as named cards and opens the chosen one', async () => {
    const onopen = vi.fn();
    const view = render(RecentHives, { sessions: hives, onopen });
    const region = view.getByRole('region', { name: 'Open hives' });
    const cards = within(region).getAllByRole('button');
    expect(cards.map(card => card.querySelector('.name')?.textContent)).toEqual(['Newest hive', 'Middle hive', 'Older hive']);
    // Every card names its hive in text; the miniature is decoration.
    for (const card of cards) expect(card.querySelector('.thumb')?.getAttribute('aria-hidden')).toBe('true');

    await fireEvent.click(cards[1]);
    expect(onopen.mock.calls).toEqual([['mid']]);
  });

  it('summarises state, calls out agents that need the operator and marks them in the miniature', () => {
    const view = render(RecentHives, { sessions: hives, onopen: vi.fn(), limit: 4 });
    const newest = view.getByRole('button', { name: /Newest hive/ });
    expect(newest.textContent).toContain('Running · 1 need you');
    expect([...newest.querySelectorAll('.thumb i')].map(cell => cell.getAttribute('data-tone')))
      .toEqual(['running', 'attention', 'empty', 'empty']);
    expect(view.getByRole('button', { name: /Older hive/ }).textContent).toContain('Completed · 1 agent');
    expect(view.getAllByRole('button')).toHaveLength(4);
  });

  it('renders nothing when no hive is open', () => {
    const view = render(RecentHives, { sessions: [], onopen: vi.fn() });
    expect(view.queryByRole('region')).toBeNull();
    expect(view.queryByRole('button')).toBeNull();
  });
});
