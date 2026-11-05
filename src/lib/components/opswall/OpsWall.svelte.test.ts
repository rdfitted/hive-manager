import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get, writable } from 'svelte/store';
import type { Session } from '$lib/stores/sessions';
import type { CoordinationMessage } from '$lib/stores/coordination';

const fixture = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('$lib/stores/events', () => ({ events: writable({ events: [] }) }));
import { coordination } from '$lib/stores/coordination';
import OpsWall from './OpsWall.svelte';

const log: CoordinationMessage[] = Array.from({ length: 45 }, (_, index) => ({
  id: `message-${index}`, timestamp: new Date(Date.UTC(2030, 0, 1, 0, 0, index)).toISOString(),
  from: 'QUEEN', to: 'all', message_type: 'Task', content: `Fixture ${index}`,
}));
const session = { id: 'ops-session', agents: [], state: 'Running' } as unknown as Session;
const props = () => ({ session, attention: new Set<string>(), stale: new Set<string>(), filter: null, onfilter: vi.fn(), onselect: vi.fn() });

beforeEach(() => {
  fixture.invoke.mockReset();
  coordination.setSessionId(null);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

it('R2-F2 mounting with a full shared log preserves it and only the ticker takes the newest 30', async () => {
  fixture.invoke.mockResolvedValueOnce(log);
  await coordination.loadLog(session.id);
  fixture.invoke.mockClear();
  const load = vi.spyOn(coordination, 'loadLog');
  const view = render(OpsWall, props());
  await tick();
  expect(load).not.toHaveBeenCalled();
  expect(fixture.invoke).not.toHaveBeenCalled();
  expect(get(coordination).log).toBe(log);
  expect(view.getByText('Task: Fixture 44')).toBeTruthy();
  expect(view.getByText('Task: Fixture 15')).toBeTruthy();
  expect(view.queryByText('Task: Fixture 14')).toBeNull();
});

it.each([{ result: log }, { result: [] }])('R2-F2 mounting without a log loads the full log once, even for an empty result (%#)', async ({ result }) => {
  fixture.invoke.mockResolvedValueOnce(result);
  const load = vi.spyOn(coordination, 'loadLog');
  render(OpsWall, props());
  await tick(); await tick();
  expect(load.mock.calls).toEqual([[session.id]]);
  expect(fixture.invoke.mock.calls).toEqual([['get_coordination_log', { sessionId: session.id, limit: undefined }]]);
  expect(get(coordination).log).toBe(result);
});

it('R2-F2 mounting while the full log is loading preserves the pending request', async () => {
  let resolve!: (rows: CoordinationMessage[]) => void;
  fixture.invoke.mockReturnValueOnce(new Promise<CoordinationMessage[]>(done => { resolve = done; }));
  const pending = coordination.loadLog(session.id);
  const load = vi.spyOn(coordination, 'loadLog');
  render(OpsWall, props());
  await tick();
  expect(load).not.toHaveBeenCalled();
  resolve(log); await pending; await tick();
  expect(load).not.toHaveBeenCalled();
  expect(get(coordination).log).toBe(log);
});

it('R2-F2 changing sessions loads the new full log instead of reusing the old session log', async () => {
  fixture.invoke.mockResolvedValueOnce(log);
  await coordination.loadLog('previous-session');
  fixture.invoke.mockResolvedValueOnce([]);
  const load = vi.spyOn(coordination, 'loadLog');
  render(OpsWall, props());
  await tick(); await tick();
  expect(load.mock.calls).toEqual([[session.id]]);
  expect(get(coordination)).toMatchObject({ sessionId: session.id, log: [], loading: false });
});
