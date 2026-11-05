import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get, writable } from 'svelte/store';
import type { Session } from '$lib/stores/sessions';
import type { CoordinationMessage } from '$lib/stores/coordination';

const fixture = vi.hoisted(() => ({ invoke: vi.fn(), onMessage: (_event: { payload: CoordinationMessage }) => {} }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn((name, callback) => {
  if (name === 'coordination-message') fixture.onMessage = callback;
  return Promise.resolve(() => {});
}) }));
vi.mock('$lib/stores/events', () => ({ events: writable({ events: [] }) }));
vi.mock('$lib/stores/sessions', async (importOriginal) => ({
  ...await importOriginal<typeof import('$lib/stores/sessions')>(),
  activeSession: writable<Session | null>(null),
}));
import { coordination } from '$lib/stores/coordination';
import { activeSession } from '$lib/stores/sessions';
import CoordinationPanel from '../CoordinationPanel.svelte';
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
  (activeSession as import('svelte/store').Writable<Session | null>).set(null);
});
afterEach(async () => {
  // Let the real Logs panel finish its deferred auto-scroll before unmounting.
  await new Promise(resolve => setTimeout(resolve, 0));
  cleanup();
  vi.restoreAllMocks();
});

it.each(['failure', 'success'] as const)('R4-F1 Logs-open session switch clears old log and ticker targets while history is pending and after %s', async (outcome) => {
  const withWorker = (id: string): Session => ({ ...session, id, agents: [{ id: `${id}-worker-1`, role: { Worker: { index: 1, parent: null } }, parent_id: null, status: 'Running', config: { cli: 'codex', flags: [] } }] });
  const alpha = withWorker('session-alpha'), beta = withWorker('session-beta');
  const alphaMessage = { ...log[0], id: 'alpha-directive', to: 'WORKER-1', content: 'Alpha directive' };
  const betaMessage = { ...log[1], id: 'beta-directive', to: 'WORKER-1', content: 'Beta directive' };
  let resolve!: (rows: CoordinationMessage[]) => void;
  let reject!: (error: Error) => void;
  const pending = new Promise<CoordinationMessage[]>((done, fail) => { resolve = done; reject = fail; });
  fixture.invoke.mockResolvedValueOnce([alphaMessage]).mockReturnValueOnce(pending);
  const displayedSession = activeSession as import('svelte/store').Writable<Session | null>;
  displayedSession.set(alpha);
  const logs = render(CoordinationPanel);
  await tick(); await tick();
  const onselect = vi.fn();
  const wall = render(OpsWall, { ...props(), session: alpha, onselect });
  await tick();
  expect(logs.getByText('Alpha directive')).toBeTruthy();
  await fireEvent.click(wall.getByRole('button', { name: 'Task: Alpha directive' }));
  expect(onselect).toHaveBeenCalledWith(alpha.agents[0].id);
  onselect.mockClear();

  // Exercise the real Logs reactive load before OpsWall's session clearing effect.
  displayedSession.set(beta);
  await tick();
  await wall.rerender({ ...props(), session: beta, onselect });
  const assertNoAlpha = () => {
    expect(get(coordination).log).toEqual([]);
    expect(logs.queryByText('Alpha directive')).toBeNull();
    expect(wall.queryByRole('button', { name: 'Task: Alpha directive' })).toBeNull();
    expect(wall.getByRole('region', { name: 'Session event ticker' }).querySelectorAll('button')).toHaveLength(0);
    expect(onselect).not.toHaveBeenCalled();
  };
  expect(get(coordination)).toMatchObject({ sessionId: beta.id, loading: true });
  assertNoAlpha();
  expect(fixture.invoke.mock.calls).toEqual([
    ['get_coordination_log', { sessionId: alpha.id, limit: undefined }],
    ['get_coordination_log', { sessionId: beta.id, limit: undefined }],
  ]);
  if (outcome === 'failure') {
    reject(new Error('Beta history unavailable'));
    await pending.catch(() => {}); await tick();
    expect(get(coordination)).toMatchObject({ loading: false, error: 'Error: Beta history unavailable' });
    expect(logs.getByText('Error: Beta history unavailable')).toBeTruthy();
    assertNoAlpha();
  } else {
    resolve([betaMessage]);
    await pending; await tick();
    expect(get(coordination)).toMatchObject({ loading: false, error: null, log: [betaMessage] });
    expect(logs.queryByText('Alpha directive')).toBeNull();
    expect(wall.queryByRole('button', { name: 'Task: Alpha directive' })).toBeNull();
    expect(logs.getByText('Beta directive')).toBeTruthy();
    await fireEvent.click(wall.getByRole('button', { name: 'Task: Beta directive' }));
    expect(onselect.mock.calls).toEqual([[beta.agents[0].id]]);
  }
});

it('R3-F1 foreign live messages cannot appear in the ticker or select this session worker', async () => {
  fixture.invoke.mockResolvedValueOnce([]);
  await coordination.loadLog(session.id);
  const onselect = vi.fn();
  const agents = [{ id: `${session.id}-worker-1`, role: { Worker: { index: 1, parent: null } }, parent_id: null, status: 'Running', config: { cli: 'codex', flags: [] } }] satisfies import('$lib/stores/sessions').AgentInfo[];
  const view = render(OpsWall, { ...props(), session: { ...session, agents }, onselect });
  const message: CoordinationMessage = { ...log[0], id: 'foreign', session_id: 'other-session', to: 'WORKER-1', content: 'Foreign directive' };
  fixture.onMessage({ payload: message });
  fixture.onMessage({ payload: { ...message, id: 'unscoped', session_id: undefined } });
  await tick();
  expect(view.queryByText('Task: Foreign directive')).toBeNull();
  expect(get(coordination).log).toEqual([]);
  expect(onselect).not.toHaveBeenCalled();
  fixture.onMessage({ payload: { ...message, id: 'local', session_id: session.id, content: 'Local directive' } });
  await tick();
  await fireEvent.click(view.getByRole('button', { name: 'Task: Local directive' }));
  expect(onselect).toHaveBeenCalledTimes(1);
  expect(onselect).toHaveBeenCalledWith(agents[0].id);
});

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
