import type { CoordinationMessage } from '$lib/stores/coordination';
import type { Event } from '$lib/types/domain';
import type { AgentInfo } from '$lib/stores/sessions';
import { agentMentions } from '$lib/composer/sources';

export interface TickerItem { id: string; at: string; agentId: string | null; text: string; }

export function tickerItems(sessionId: string, agents: AgentInfo[], log: CoordinationMessage[], events: Event[]): TickerItem[] {
  const labels = agentMentions(agents);
  const resolve = (value: string | undefined) => agents.find(agent => agent.id === value)?.id
    ?? labels.find(agent => agent.label === value)?.id ?? null;
  return [
    ...log.map(row => ({ id: `coord:${row.id}`, at: row.timestamp, agentId: resolve(row.to) ?? resolve(row.from), text: `${row.message_type}: ${row.content}` })),
    ...events.filter(row => row.session_id === sessionId).map(row => ({ id: `event:${row.id}`, at: row.timestamp, agentId: resolve(row.agent_id), text: row.event_type.replaceAll('_', ' ') })),
  ].sort((a, b) => b.at.localeCompare(a.at) || a.id.localeCompare(b.id)).slice(0, 30);
}
