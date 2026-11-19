import type { CoordinationMessage } from '$lib/stores/coordination';
import type { Event } from '$lib/types/domain';
import type { AgentInfo } from '$lib/stores/sessions';
import { agentMentions } from '$lib/composer/sources';
import { apiUrl } from '$lib/config';
import type { ArtifactBundle, Cell } from '$lib/types/domain';
import type { WorkGraphResponse } from '$lib/workgraph/types';

export interface TickerItem { id: string; at: string; agentId: string | null; text: string; }
const timestamp = (value: string) => { const parsed = Date.parse(value); return Number.isFinite(parsed) ? parsed : Number.NEGATIVE_INFINITY; };
/** Match the backend coordination log's format_agent_display wire names. */
function coordinationName(id: string): string {
  if (id.endsWith('-queen')) return 'QUEEN';
  if (id.endsWith('-evaluator')) return 'EVALUATOR';
  for (const [marker, prefix] of [['-qa-worker-', 'QA-WORKER-'], ['-worker-', 'WORKER-'], ['-planner-', 'PLANNER-']]) {
    const index = id.lastIndexOf(marker);
    if (index >= 0) {
      const suffix = id.slice(index + marker.length);
      return prefix + (marker === '-planner-' ? suffix.split('-')[0] : suffix);
    }
  }
  return id;
}

export function tickerItems(sessionId: string, agents: AgentInfo[], log: CoordinationMessage[], events: Event[]): TickerItem[] {
  const labels = agentMentions(agents);
  const resolve = (value: string | undefined) => {
    const exact = agents.find(agent => agent.id === value);
    if (exact) return exact.id;
    const aliases = agents.filter(agent => coordinationName(agent.id) === value);
    if (aliases.length) return aliases.length === 1 ? aliases[0].id : null;
    const named = labels.filter(agent => agent.label === value);
    return named.length === 1 ? named[0].id : null;
  };
  return [
    ...log.map(row => ({ id: `coord:${row.id}`, at: row.timestamp, agentId: resolve(row.to) ?? resolve(row.from), text: `${row.message_type}: ${row.content}` })),
    ...events.filter(row => row.session_id === sessionId).map(row => ({ id: `event:${row.id}`, at: row.timestamp, agentId: resolve(row.agent_id), text: row.event_type.replaceAll('_', ' ') })),
  ].sort((a, b) => timestamp(b.at) - timestamp(a.at) || a.id.localeCompare(b.id)).slice(0, 30);
}

export function activityLanes(agents: AgentInfo[], items: TickerItem[]) {
  const timed = items.flatMap(item => {
    const at = Date.parse(item.at);
    return item.agentId && Number.isFinite(at) ? [{ ...item, at }] : [];
  });
  const newest = Math.max(...timed.map(item => item.at));
  const oldest = Math.min(newest - 60_000, ...timed.map(item => item.at));
  return agentMentions(agents).map(agent => ({
    ...agent,
    marks: timed.filter(item => item.agentId === agent.id).map(item => ({
      ...item,
      position: 2 + 96 * (item.at - oldest) / (newest - oldest),
    })),
  }));
}

async function readBandData<T>(path: string, signal: AbortSignal): Promise<T> {
  const response = await fetch(apiUrl(path), { signal });
  if (!response.ok) throw new Error(`Request failed (${response.status})`);
  return response.json();
}

export async function bandNodeCount(sessionId: string, signal: AbortSignal): Promise<string> {
  const path = `/api/sessions/${encodeURIComponent(sessionId)}/work-graph?view=runtime`;
  const graph = await readBandData<WorkGraphResponse>(path, signal);
  const complete = graph.nodes.filter(node => graph.status_by_node[node.id] === 'completed').length;
  return `${complete}/${graph.nodes.length}`;
}

export async function bandFileCount(sessionId: string, signal: AbortSignal): Promise<number> {
  const path = `/api/sessions/${encodeURIComponent(sessionId)}/cells`;
  const cells = await readBandData<Pick<Cell, 'id'>[]>(path, signal);
  const bundles = await Promise.all(cells.map(cell =>
    readBandData<ArtifactBundle[]>(`${path}/${encodeURIComponent(cell.id)}/artifacts`, signal)
  ));
  return new Set(bundles.flat().flatMap(bundle => bundle.changed_files)).size;
}
