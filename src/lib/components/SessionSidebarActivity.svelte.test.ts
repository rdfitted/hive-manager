// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get, type Writable } from 'svelte/store';
import type { AgentInfo, Session } from '$lib/stores/sessions';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue([]) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('$app/stores', async () => ({ page: (await import('svelte/store')).writable({ url: new URL('http://localhost/') }) }));
vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    sessions: { ...writable({ sessions: [] }), setActiveSession: vi.fn(), closeSession: vi.fn() },
    activeSession: writable(null),
    activeAgents: writable([]),
    sessionDisplayName: (session: Session) => session.name || session.id,
    serdeEnumVariantName: (value: unknown) => typeof value === 'string' ? value
      : value && typeof value === 'object' ? Object.keys(value)[0] : undefined,
  };
});
vi.mock('./QueenControls.svelte', () => ({ default: () => {} }));
vi.mock('./ResumeConfirmModal.svelte', () => ({ default: () => {} }));

import SessionSidebar from './SessionSidebar.svelte';
import { sessions, activeSession, activeAgents } from '$lib/stores/sessions';
import { layout } from '$lib/stores/layout';

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it('g52_sidebar_without_selection_renders_all_stale_badges_with_one_fetch_per_tick', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-04T12:00:00Z'));
  (activeSession as Writable<Session | null>).set(null);
  (activeAgents as Writable<AgentInfo[]>).set([]);
  const listed = ['Alpha', 'Beta'].map(name => ({
    id: name.toLowerCase(), name: `${name} hive`, project_path: `D:/synthetic/${name}`,
    state: 'Running', session_type: { Hive: { worker_count: 1 } },
    agents: [{ id: `${name.toLowerCase()}-worker`, status: 'Running', work_state: 'working' }],
  })) as unknown as Session[];
  (sessions as unknown as Writable<{ sessions: Session[] }>).set({ sessions: listed });
  if (get(layout).leftCollapsed) layout.toggleLeft();
  if (get(layout).sessionsCollapsed) layout.toggleSection('sessionsCollapsed');
  let stale = true;
  const fetchMock = vi.fn().mockImplementation(async () => ({
    ok: true,
    json: async () => ({ sessions: listed.map(session => ({
      id: session.id, agents: [{ id: session.agents[0].id, work_state: 'working',
        last_activity: new Date(Date.now() - (stale ? 4 * 60_000 : 0)).toISOString() }],
    })) }),
  }));
  vi.stubGlobal('fetch', fetchMock);
  // No Chat, agent tree, direct activity subscription, or mocked heartbeat loader:
  // the sidebar alone starts the real shared loader and consumes both session maps.
  const view = render(SessionSidebar);
  await vi.advanceTimersByTimeAsync(0);
  await tick();
  expect(get(activeSession)).toBeNull();
  expect(view.queryByRole('tree', { name: 'Agent hierarchy' })).toBeNull();
  expect(fetchMock).toHaveBeenCalledTimes(1);
  expect(fetchMock).toHaveBeenLastCalledWith(expect.stringContaining('/api/sessions/active'));
  for (const name of ['Alpha', 'Beta']) {
    expect(view.getByRole('button', { name: `1 need attention in ${name} hive` })).toBeTruthy();
  }
  stale = false;
  await vi.advanceTimersByTimeAsync(10_000);
  await tick();
  expect(fetchMock).toHaveBeenCalledTimes(2);
  for (const name of ['Alpha', 'Beta']) {
    expect(view.queryByRole('button', { name: `1 need attention in ${name} hive` })).toBeNull();
  }
  stale = true;
  await vi.advanceTimersByTimeAsync(10_000);
  await tick();
  expect(fetchMock).toHaveBeenCalledTimes(3);
  for (const name of ['Alpha', 'Beta']) {
    expect(view.getByRole('button', { name: `1 need attention in ${name} hive` })).toBeTruthy();
  }
  expect(get(activeSession)).toBeNull();
  view.unmount();
  await vi.advanceTimersByTimeAsync(30_000);
  expect(fetchMock).toHaveBeenCalledTimes(3);
  expect(get(activeSession)).toBeNull();
});
