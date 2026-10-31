import { readable } from 'svelte/store';
import { apiUrl } from '$lib/config';
import { getDependencyEdges, getNodeAdjacency } from '$lib/workgraph/graphUtils';
import type { WorkGraphResponse } from '$lib/workgraph/types';
import { activeSession } from './sessions';

export const DEPENDENCY_POLL_MS = 3000;

export interface DepRef {
  nodeId: string;
  title: string;
  agentId: string | null;
}

export interface AgentDependencies {
  waitingOn: DepRef[];
  blocks: DepRef[];
}

export interface DependencyState {
  byAgent: Map<string, AgentDependencies>;
}

export function dependencyState(graph: WorkGraphResponse): DependencyState {
  const byAgent = new Map<string, AgentDependencies>();
  const nodes = new Map(graph.nodes.map((node) => [node.id, node]));
  const unfinished = (id: string) => {
    const node = nodes.get(id);
    const status = graph.status_by_node[id] ?? node?.status;
    return node !== undefined && status !== 'completed' && status !== 'cancelled';
  };
  const edges = getDependencyEdges(graph.edges).filter(
    (edge) => unfinished(edge.source) && unfinished(edge.target),
  );
  const ref = (id: string): DepRef => {
    const node = nodes.get(id)!;
    return { nodeId: id, title: node.title, agentId: node.progress?.agent_id ?? null };
  };
  for (const node of graph.nodes) {
    const agentId = node.progress?.agent_id;
    if (!agentId) continue;
    const entry = byAgent.get(agentId) ?? { waitingOn: [], blocks: [] };
    const adjacency = getNodeAdjacency(node.id, edges);
    for (const [key, ids] of [
      ['waitingOn', adjacency.dependencies], ['blocks', adjacency.dependents],
    ] as const) {
      for (const id of ids) {
        // Multiple work nodes may belong to one principal. Do not link it to itself.
        if (nodes.get(id)?.progress?.agent_id === agentId) continue;
        if (!entry[key].some((item) => item.nodeId === id)) entry[key].push(ref(id));
      }
    }
    byAgent.set(agentId, entry);
  }
  return { byAgent };
}

/** One subscriber-owned loop, regardless of how many rows consume its snapshot. */
export const agentDependencies = readable<DependencyState>({ byAgent: new Map() }, (set) => {
  let sessionId: string | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let controller: AbortController | undefined;
  let generation = 0;

  const unsubscribe = activeSession.subscribe((session) => {
    const nextId = session?.id ?? null;
    if (nextId === sessionId) return;
    sessionId = nextId;
    const current = ++generation;
    clearTimeout(timer);
    controller?.abort();
    set({ byAgent: new Map() });
    if (!nextId) return;
    const request = new AbortController();
    controller = request;

    async function refresh() {
      let snapshot: DependencyState = { byAgent: new Map() };
      try {
        const response = await fetch(apiUrl(`/api/sessions/${encodeURIComponent(nextId!)}/work-graph?view=runtime`), {
          signal: request.signal,
        });
        if (response.ok) snapshot = dependencyState(await response.json());
      } catch {
        // Missing, unreadable and unavailable graphs all mean delegation-only UI.
      }
      if (generation !== current) return;
      set(snapshot);
      timer = setTimeout(() => void refresh(), DEPENDENCY_POLL_MS);
    }
    void refresh();
  });

  return () => {
    generation += 1;
    clearTimeout(timer);
    controller?.abort();
    unsubscribe();
  };
});
