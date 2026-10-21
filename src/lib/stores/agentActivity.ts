import { derived, readable, type Readable } from 'svelte/store';
import { activeAgents, type AgentInfo } from './sessions';
import { heartbeatStore } from './conversations';
import { events } from './events';

export type Attention = 'none' | 'delivery_failed' | 'stale' | 'error';

export interface AgentActivity {
  agentId: string;
  summary: string;
  lastActivityAt: number | null;
  latestEventLabel: string | null;
  attention: Attention;
}

/** Reserved adapter input for the structured protocol feed in #362. */
export interface ProtocolActivityInput {
  agentId: string;
  summary?: string;
  lastActivityAt?: number;
  latestEventLabel?: string;
}

export function attentionReason(agent: AgentInfo, stale: Set<string>): Attention {
  const status = agent.status;
  if (typeof status === 'object' && status !== null) {
    if ('WaitingForInput' in status && status.WaitingForInput.startsWith('injected_delivery_failed:')) {
      return 'delivery_failed';
    }
    if ('Error' in status) return 'error';
  }
  return stale.has(agent.id) ? 'stale' : 'none';
}

export function needsAttention(agent: AgentInfo, stale: Set<string>): boolean {
  return attentionReason(agent, stale) !== 'none';
}

/** Svelte starts this once for all subscribers and stops it after the last leaves. */
export const now: Readable<number> = readable(Date.now(), (set) => {
  set(Date.now());
  const timer = setInterval(() => set(Date.now()), 30_000);
  return () => clearInterval(timer);
});

function timestamp(value: string | undefined): number | null {
  const parsed = value ? Date.parse(value) : NaN;
  return Number.isFinite(parsed) ? parsed : null;
}

export const agentActivity: Readable<Record<string, AgentActivity>> = derived(
  [activeAgents, heartbeatStore, events],
  ([$agents, $heartbeats, $events]) => {
    const latest = new Map<string, { at: number; label: string }>();
    for (const event of $events.events) {
      const at = timestamp(event.timestamp);
      if (!event.agent_id || at === null) continue;
      const previous = latest.get(event.agent_id);
      if (!previous || at > previous.at) {
        latest.set(event.agent_id, { at, label: event.event_type.replaceAll('_', ' ') });
      }
    }
    return Object.fromEntries($agents.map((agent) => {
      const heartbeat = $heartbeats.agents[agent.id];
      const heartbeatAt = timestamp(heartbeat?.timestamp);
      const event = latest.get(agent.id);
      return [agent.id, {
        agentId: agent.id,
        summary: (heartbeat?.summary ?? '').replace(/\s+/g, ' ').trim(),
        lastActivityAt: heartbeatAt === null ? event?.at ?? null : Math.max(heartbeatAt, event?.at ?? heartbeatAt),
        latestEventLabel: event?.label ?? null,
        attention: attentionReason(agent, $heartbeats.staleAgents),
      }];
    }));
  },
);
