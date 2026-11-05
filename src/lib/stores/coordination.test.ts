import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import type { CoordinationMessage } from './coordination';
const fixture = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
import { coordination } from './coordination';

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
