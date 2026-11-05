import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import type { CoordinationMessage } from './coordination';
const fixture = vi.hoisted(() => ({
  invoke: vi.fn(),
  onMessage: (_event: { payload: CoordinationMessage }) => {},
  onVariant: (_event: { payload: { variant: string } }) => {},
  onJudge: (_event: { payload: { report: string } }) => {},
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn((name, callback) => {
  if (name === 'coordination-message') fixture.onMessage = callback;
  if (name === 'fusion-variant-completed') fixture.onVariant = callback;
  if (name === 'judge-evaluation-ready') fixture.onJudge = callback;
  return Promise.resolve(() => {});
}) }));
import { coordination } from './coordination';
import { tickerItems } from '$lib/components/opswall/ticker';
import { transcriptRows } from '$lib/components/chat/transcript';

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (cause: Error) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
function message(id: string): CoordinationMessage {
  return { id, timestamp: '2026-01-01T00:00:00Z', from: 'QUEEN', to: 'WORKER-1', content: id, message_type: 'Task' };
}
beforeEach(() => { fixture.invoke.mockReset(); coordination.setSessionId(null); coordination.clearLog(); coordination.clearError(); });

describe('coordination log ownership', () => {
  it('R4-F1 loadLog clears all prior-session state before publishing the new owner', async () => {
    const prior = message('Alpha directive');
    const worker = { id: 'alpha-worker-1', role: { role_type: 'Worker', label: 'Worker', default_cli: 'codex', prompt_template: null }, cli: 'codex', status: 'Running', current_task: null, last_update: prior.timestamp };
    fixture.invoke.mockResolvedValueOnce([prior]).mockResolvedValueOnce([worker]);
    await coordination.loadLog('alpha');
    await coordination.loadWorkers('alpha');
    fixture.onVariant({ payload: { variant: 'alpha-variant' } });
    fixture.onJudge({ payload: { report: 'Alpha report' } });
    fixture.invoke.mockRejectedValueOnce(new Error('Alpha error'));
    await coordination.loadLog('alpha');
    expect(get(coordination)).toMatchObject({ log: [prior], workers: [worker], error: 'Error: Alpha error', fusionState: { completedVariants: ['alpha-variant'], judgeReport: 'Alpha report', evaluationReady: true } });

    const pending = deferred<CoordinationMessage[]>();
    fixture.invoke.mockReturnValueOnce(pending.promise);
    const ownedStates: Parameters<Parameters<typeof coordination.subscribe>[0]>[0][] = [];
    const unsubscribe = coordination.subscribe(state => { if (state.sessionId === 'beta') ownedStates.push(state); });
    try {
      const loading = coordination.loadLog('beta');
      // Logs loads first; later consumers must preserve that request and cannot repair a leaked owner.
      coordination.setSessionId('beta');
      await coordination.ensureLog('beta');
      expect(get(coordination)).toMatchObject({ sessionId: 'beta', loading: true, error: null });
      pending.reject(new Error('Beta history unavailable'));
      await loading;
      expect(get(coordination)).toMatchObject({ sessionId: 'beta', loading: false, error: 'Error: Beta history unavailable' });
      expect(ownedStates.length).toBeGreaterThan(0);
      for (const state of ownedStates) {
        expect(state).toMatchObject({ log: [], workers: [], fusionState: { completedVariants: [], judgeReport: null, evaluationReady: false } });
      }
      fixture.invoke.mockResolvedValueOnce([message('Beta directive')]);
      await coordination.ensureLog('beta');
      expect(get(coordination).log).toEqual([message('Beta directive')]);
    } finally {
      unsubscribe();
    }
  });

  it('R3-F1 foreign or unscoped live messages cannot enter the displayed log, ticker targets, or sender attribution', async () => {
    fixture.invoke.mockResolvedValueOnce([]);
    await coordination.loadLog('session-alpha');
    const foreign = { ...message('foreign'), session_id: 'session-beta', content: 'Shared text' };
    fixture.onMessage({ payload: foreign });
    fixture.onMessage({ payload: { ...message('legacy'), content: 'Shared text' } });
    expect(get(coordination).log).toEqual([]);
    const agents = [{ id: 'session-alpha-worker-1', role: { Worker: { index: 1, parent: null } }, parent_id: null, status: 'Running', config: { cli: 'codex', flags: [] } }] satisfies import('./sessions').AgentInfo[];
    expect(tickerItems('session-alpha', agents, get(coordination).log, [])).toEqual([]);
    const events = [{ agent_id: agents[0].id, cursor: 'input', kind: 'user_message', payload: { text: 'Shared text' }, source_id: 'input', ts: foreign.timestamp, truncated: false }] satisfies import('./agentEvents').AgentEvent[];
    expect(transcriptRows(events, { agentId: agents[0].id, isQueen: false, log: get(coordination).log })[0]).not.toHaveProperty('sender', 'queen');
    fixture.onMessage({ payload: { ...foreign, id: 'local', session_id: 'session-alpha' } });
    expect(tickerItems('session-alpha', agents, get(coordination).log, [])[0].agentId).toBe(agents[0].id);
    expect(transcriptRows(events, { agentId: agents[0].id, isQueen: false, log: get(coordination).log })[0]).toMatchObject({ type: 'user', sender: 'queen' });
  });

  it.each([{ rows: [] }, { rows: [message('loaded')] }])('R3-F2 ensureLog shares pending and loaded full history, including empty results (%#)', async ({ rows }) => {
    const pending = deferred<CoordinationMessage[]>(); fixture.invoke.mockReturnValueOnce(pending.promise);
    const first = coordination.ensureLog('same'); coordination.ensureLog('same');
    expect(fixture.invoke.mock.calls).toEqual([['get_coordination_log', { sessionId: 'same', limit: undefined }]]);
    pending.resolve(rows); await first;
    await coordination.ensureLog('same');
    expect(fixture.invoke).toHaveBeenCalledTimes(1);
    expect(get(coordination).log).toEqual(rows);
  });

  it('R3-F2 ensureLog loads full history after a limited load or a session change and retries current errors', async () => {
    fixture.invoke.mockResolvedValueOnce([message('limited')]);
    await coordination.loadLog('same', 30);
    fixture.invoke.mockResolvedValueOnce([message('full')]);
    await coordination.ensureLog('same');
    expect(get(coordination).log).toEqual([message('full')]);
    fixture.invoke.mockRejectedValueOnce(new Error('temporary'));
    await coordination.ensureLog('other');
    fixture.invoke.mockResolvedValueOnce([]);
    await coordination.ensureLog('other'); await coordination.ensureLog('other');
    expect(fixture.invoke.mock.calls.map(([, args]) => args)).toEqual([
      { sessionId: 'same', limit: 30 }, { sessionId: 'same', limit: undefined },
      { sessionId: 'other', limit: undefined }, { sessionId: 'other', limit: undefined },
    ]);
  });
  it.each(['success', 'error'] as const)('latest request owns the same-session log after an older %s', async (outcome) => {
    const old = deferred<CoordinationMessage[]>(), fresh = deferred<CoordinationMessage[]>();
    fixture.invoke.mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
    const first = coordination.loadLog('same'), second = coordination.loadLog('same');
    fresh.resolve([message('fresh')]); await second;
    if (outcome === 'success') old.resolve([message('old')]); else old.reject(new Error('old failure'));
    await first;
    expect(get(coordination)).toMatchObject({ sessionId: 'same', log: [message('fresh')], loading: false, error: null });
  });
  it('ignores an old-session response after the new session finishes', async () => {
    const old = deferred<CoordinationMessage[]>(), fresh = deferred<CoordinationMessage[]>();
    fixture.invoke.mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
    const first = coordination.loadLog('alpha'), second = coordination.loadLog('beta', 30);
    fresh.resolve([message('beta')]); await second;
    old.resolve([message('alpha')]); await first;
    expect(get(coordination)).toMatchObject({ sessionId: 'beta', log: [message('beta')], loading: false, error: null });
    expect(fixture.invoke).toHaveBeenLastCalledWith('get_coordination_log', { sessionId: 'beta', limit: 30 });
  });
  it('session changes invalidate a pending response even after returning to the old id', async () => {
    const pending = deferred<CoordinationMessage[]>(); fixture.invoke.mockReturnValueOnce(pending.promise);
    const first = coordination.loadLog('alpha');
    coordination.setSessionId('beta'); coordination.setSessionId('alpha');
    pending.resolve([message('old-alpha')]); await first;
    expect(get(coordination)).toMatchObject({ sessionId: 'alpha', log: [], loading: false, error: null });
  });
  it('an older result cannot clear loading while the current request is pending', async () => {
    const old = deferred<CoordinationMessage[]>(), fresh = deferred<CoordinationMessage[]>();
    fixture.invoke.mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
    const first = coordination.loadLog('same'), second = coordination.loadLog('same');
    old.resolve([message('old')]); await first;
    expect(get(coordination)).toMatchObject({ log: [], loading: true, error: null });
    fresh.resolve([message('fresh')]); await second;
    expect(get(coordination)).toMatchObject({ log: [message('fresh')], loading: false });
  });
  it('selecting the current session preserves its pending request', async () => {
    const pending = deferred<CoordinationMessage[]>(); fixture.invoke.mockReturnValueOnce(pending.promise);
    const first = coordination.loadLog('same'); coordination.setSessionId('same');
    pending.resolve([message('same')]); await first;
    expect(get(coordination)).toMatchObject({ log: [message('same')], loading: false });
  });
  it('reports an error from the current request', async () => {
    fixture.invoke.mockRejectedValueOnce(new Error('current failure'));
    await coordination.loadLog('same');
    expect(get(coordination)).toMatchObject({ sessionId: 'same', loading: false, error: 'Error: current failure' });
  });
});
