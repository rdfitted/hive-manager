<script lang="ts">
  import type { AgentInfo } from '$lib/stores/sessions';
  import { needsAttention, attentionReason, startupExitCode, isFinished, workStateFor } from '$lib/stores/agentActivity';
  import { heartbeatStore } from '$lib/stores/conversations';
  import { layout } from '$lib/stores/layout';
  import { ui } from '$lib/stores/ui';
  import { createFlushState, flushKey, type FlushState } from './flushState';
  import FlushReceipt from './FlushReceipt.svelte';

  let { sessionId, agent, hasTerminal = true, chatAvailable = false, eventAttention = null, feedError = null, onOpen, flushState = createFlushState() }: {
    sessionId: string; agent: AgentInfo; hasTerminal?: boolean; chatAvailable?: boolean;
    eventAttention?: string | null; feedError?: string | null; onOpen?: () => void; flushState?: FlushState;
  } = $props();
  const waitingForInput = $derived(chatAvailable && typeof agent.status === 'object' && agent.status !== null
    && 'WaitingForInput' in agent.status && !agent.status.WaitingForInput.startsWith('awaiting_injected_turn'));
  const workState = $derived(workStateFor(agent, $heartbeatStore.agents[agent.id]));
  const finished = $derived(isFinished(agent, workState));
  const visible = $derived(needsAttention(agent, $heartbeatStore.staleAgents, workState)
    || (!finished && (waitingForInput || !!eventAttention || !!feedError)));
  const reason = $derived(attentionReason(agent, $heartbeatStore.staleAgents, workState));
  const exitCode = $derived(startupExitCode(agent));
  const reasons = {
    delivery_failed: 'The last message could not be delivered.',
    stale: 'No heartbeat has arrived for more than three minutes.',
    error: 'This agent reported an error.',
    none: '',
  };
  const currentResult = $derived($flushState[flushKey(sessionId, agent.id)]);
  const pending = $derived(currentResult?.pending ?? false);

  function openTerminal() {
    if (onOpen) { onOpen(); return; }
    layout.setSessionViewMode('focus');
    ui.setFocusedAgent(agent.id);
  }
</script>

{#if visible}
  <section class="attention lattice-forced-colors-boundary" aria-label="Agent needs attention">
    <p class="reason" role={chatAvailable || exitCode !== null ? 'status' : undefined}>{exitCode !== null ? `This agent's process exited during startup with code ${exitCode}. It has not been restarted.` : reasons[reason] || eventAttention || feedError || (hasTerminal ? 'Agent needs input. A prompt may only be visible in the terminal.' : 'Agent needs input. Check the raw event log.')}</p>
    <div class="actions">
      {#if hasTerminal && !finished && reason !== 'error'}<button type="button" class="lattice-btn lattice-btn--secondary" disabled={pending} onclick={() => flushState.flush(sessionId, agent.id)}>{pending ? 'Flushing…' : 'Flush Enter'}</button>{/if}
      <button type="button" class="lattice-btn lattice-btn--ghost" onclick={openTerminal}>{hasTerminal ? 'Open terminal' : 'Open raw event log'}</button>
    </div>
    {#if currentResult?.receipt !== undefined}<FlushReceipt body={currentResult.receipt} ondismiss={() => flushState.dismiss(sessionId, agent.id)} />{/if}
    {#if currentResult?.error}<p role="alert">{currentResult.error}</p>{/if}
  </section>
{/if}

<style>
  .attention { padding: var(--space-3) var(--space-4); background: var(--surface); border-radius: var(--radius-md); box-shadow: var(--depth-1); }
  .reason { margin: 0 0 var(--space-2); color: var(--text-primary); font-size: var(--text-small); }
  .actions { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); }
</style>
