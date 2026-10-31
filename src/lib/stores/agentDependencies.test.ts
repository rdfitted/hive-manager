import workGraphSource from '$lib/components/workgraph/WorkGraphView.svelte?raw';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get, type Writable } from 'svelte/store';
import type { Session } from './sessions';
import type { NodeStatus, WorkGraphNode, WorkGraphResponse } from '$lib/workgraph/types';

vi.mock('./sessions', async () => ({ activeSession: (await import('svelte/store')).writable(null) }));
import { activeSession } from './sessions';
import { agentDependencies, dependencyState, DEPENDENCY_POLL_MS } from './agentDependencies';

function node(id: string, agentId: string | null, status: NodeStatus = 'pending'): WorkGraphNode {
  return { id, title: `Task ${id}`, status, progress: { agent_id: agentId } } as WorkGraphNode;
}
function graph(): WorkGraphResponse {
  return {
    nodes: [node('base', 'foundation'), node('worker', 'frontend'), node('future', null), node('info', 'informant')],
    edges: [
      { source: 'base', target: 'worker', kind: 'depends_on', provenance: 'planner' },
      { source: 'worker', target: 'future', kind: 'depends_on', provenance: 'planner' },
      { source: 'info', target: 'worker', kind: 'informs', provenance: 'planner' },
      { source: 'worker', target: 'info', kind: 'touches', provenance: 'planner' },
    ], status_by_node: {},
  } as WorkGraphResponse;
}
const activate = (id: string | null) => (activeSession as Writable<Session | null>).set(id ? { id } as Session : null);
const flush = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };
const subscriptions: (() => void)[] = [];
const listen = () => { const stop = agentDependencies.subscribe(() => {}); subscriptions.push(stop); return stop; };

beforeEach(() => { vi.useFakeTimers(); activate(null); });
afterEach(() => {
  subscriptions.splice(0).forEach((stop) => stop());
  activate(null); vi.unstubAllGlobals(); vi.useRealTimers();
});

describe('agent dependency mapping', () => {
  it('counts only depends_on, in prerequisite-to-dependent direction, preserving null-agent text refs', () => {
    const { byAgent } = dependencyState(graph());
    expect(byAgent.get('frontend')).toEqual({
      waitingOn: [{ nodeId: 'base', title: 'Task base', agentId: 'foundation' }],
      blocks: [{ nodeId: 'future', title: 'Task future', agentId: null }],
    });
    expect(byAgent.get('informant')).toEqual({ waitingOn: [], blocks: [] });
  });
  it('removes satisfied prerequisites and finished dependents, using runtime status overrides', () => {
    const data = graph();
    data.status_by_node = { base: 'completed', future: 'cancelled' };
    expect(dependencyState(data).byAgent.get('frontend')).toEqual({ waitingOn: [], blocks: [] });
  });
  it('deduplicates refs, skips dangling nodes and does not claim same-principal tasks block themselves', () => {
    const data = graph();
    data.nodes.push(node('own', 'frontend'));
    data.edges.push(data.edges[0],
      { source: 'own', target: 'worker', kind: 'depends_on', provenance: 'planner' },
      { source: 'missing', target: 'worker', kind: 'depends_on', provenance: 'planner' });
    expect(dependencyState(data).byAgent.get('frontend')?.waitingOn.map((item) => item.nodeId)).toEqual(['base']);
  });
  it('never polls faster than WorkGraphView', () => {
    const match = workGraphSource.match(/const POLL_MS = (\d+);/);
    expect(match).not.toBeNull();
    expect(DEPENDENCY_POLL_MS).toBe(3000);
    expect(DEPENDENCY_POLL_MS).toBeGreaterThanOrEqual(Number(match![1]));
  });
});

describe('active-session dependency polling', () => {
  it('shares exactly one runtime poll across subscribers and stops/replaces it on session change', async () => {
    const fetcher = vi.fn().mockResolvedValue({ ok: true, json: async () => graph() });
    vi.stubGlobal('fetch', fetcher);
    listen(); listen();
    expect(fetcher).not.toHaveBeenCalled();
    activate('one'); await flush();
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(fetcher.mock.calls[0][0]).toMatch(/\/sessions\/one\/work-graph\?view=runtime$/);
    expect(vi.getTimerCount()).toBe(1);
    await vi.advanceTimersByTimeAsync(DEPENDENCY_POLL_MS);
    expect(fetcher).toHaveBeenCalledTimes(2);
    const oldSignal = fetcher.mock.calls[0][1].signal as AbortSignal;
    activate('two'); await flush();
    expect(oldSignal.aborted).toBe(true);
    expect(fetcher).toHaveBeenCalledTimes(3);
    expect(vi.getTimerCount()).toBe(1);
    activate(null); await vi.advanceTimersByTimeAsync(DEPENDENCY_POLL_MS * 2);
    expect(fetcher).toHaveBeenCalledTimes(3);
    expect(get(agentDependencies).byAgent.size).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });
  it.each([404, 500, 'thrown'] as const)('maps %s to empty without exposing an error, including after good data', async (failure) => {
    const fetcher = vi.fn().mockResolvedValueOnce({ ok: true, json: async () => graph() });
    if (failure === 'thrown') fetcher.mockRejectedValue(new Error('offline'));
    else fetcher.mockResolvedValue({ ok: false, status: failure });
    vi.stubGlobal('fetch', fetcher); activate('session'); listen(); await flush();
    expect(get(agentDependencies).byAgent.size).toBeGreaterThan(0);
    await vi.advanceTimersByTimeAsync(DEPENDENCY_POLL_MS);
    expect(get(agentDependencies)).toEqual({ byAgent: new Map() });
  });
  it('aborts stale requests, ignores late results and leaves no timer after the last unsubscribe', async () => {
    let resolve!: (value: unknown) => void;
    const fetcher = vi.fn().mockImplementationOnce(() => new Promise((done) => { resolve = done; }))
      .mockResolvedValue({ ok: false, status: 404 });
    vi.stubGlobal('fetch', fetcher); activate('old'); const stop = listen();
    await vi.advanceTimersByTimeAsync(DEPENDENCY_POLL_MS * 2);
    expect(fetcher).toHaveBeenCalledTimes(1); // An unresolved poll cannot overlap itself.
    activate('new'); await flush();
    resolve({ ok: true, json: async () => graph() }); await flush();
    expect(get(agentDependencies)).toEqual({ byAgent: new Map() });
    expect(vi.getTimerCount()).toBe(1);
    stop();
    expect(fetcher.mock.calls[1][1].signal.aborted).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
  });
});
