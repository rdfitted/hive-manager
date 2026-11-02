import type { AgentInfo } from '$lib/stores/sessions';
import { isFinished, needsAttention } from '$lib/stores/agentActivity';

/** Shared by the card and its wrapper so an empty card never leaves an overlay. */
export function attentionVisible(agent: AgentInfo, staleAgents: Set<string>, workState: string | undefined,
  { chatAvailable = false, eventAttention = null, feedError = null }: {
    chatAvailable?: boolean; eventAttention?: string | null; feedError?: string | null;
  } = {}): boolean {
  const waitingForInput = chatAvailable && typeof agent.status === 'object' && agent.status !== null
    && 'WaitingForInput' in agent.status && !agent.status.WaitingForInput.startsWith('awaiting_injected_turn');
  return needsAttention(agent, staleAgents, workState)
    || (!isFinished(agent, workState) && (waitingForInput || !!eventAttention || !!feedError));
}
