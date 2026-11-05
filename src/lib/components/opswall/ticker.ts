import type { CoordinationMessage } from '$lib/stores/coordination';
import type { Event } from '$lib/types/domain';
import type { AgentInfo } from '$lib/stores/sessions';
import { agentMentions } from '$lib/composer/sources';

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
