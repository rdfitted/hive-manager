<script lang="ts">
  import type { AgentInfo } from '$lib/stores/sessions';
  import { needsAttention, attentionReason } from '$lib/stores/agentActivity';
  import { heartbeatStore } from '$lib/stores/conversations';
  import { layout } from '$lib/stores/layout';
  import { ui } from '$lib/stores/ui';
  import { flushEnter } from './flush';
  import FlushReceipt from './FlushReceipt.svelte';

  let { sessionId, agent, hasTerminal = true, chatAvailable = false, eventAttention = null, feedError = null, onOpen }: {
    sessionId: string; agent: AgentInfo; hasTerminal?: boolean; chatAvailable?: boolean;
    eventAttention?: string | null; feedError?: string | null; onOpen?: () => void;
  } = $props();
  const waitingForInput = $derived(chatAvailable && typeof agent.status === 'object' && agent.status !== null
    && 'WaitingForInput' in agent.status && !agent.status.WaitingForInput.startsWith('awaiting_injected_turn'));
  const visible = $derived(needsAttention(agent, $heartbeatStore.staleAgents) || waitingForInput || !!eventAttention || !!feedError);
  const reason = $derived(attentionReason(agent, $heartbeatStore.staleAgents));
  const reasons = {
    delivery_failed: 'The last message could not be delivered.',
    stale: 'No heartbeat has arrived for more than three minutes.',
    error: 'This agent reported an error.',
    none: '',
  };
  let pending = $state(false);
  let result = $state<{ sessionId: string; agentId: string; receipt?: string; error?: string } | null>(null);
  const currentResult = $derived(result?.sessionId === sessionId && result?.agentId === agent.id ? result : null);

  async function flush() {
    if (pending) return;
    const id = agent.id;
    const session = sessionId;
    pending = true;
    result = null;
    try {
      result = { sessionId: session, agentId: id, receipt: await flushEnter(session, id) };
    } catch (cause) {
      result = { sessionId: session, agentId: id, error: String(cause) };
    } finally { pending = false; }
  }

  function openTerminal() {
    if (onOpen) { onOpen(); return; }
    layout.setSessionViewMode('focus');
    ui.setFocusedAgent(agent.id);
  }
</script>

{#if visible}
  <section class="attention lattice-forced-colors-boundary" aria-label="Agent needs attention">
    <p class="reason" role={chatAvailable ? 'status' : undefined}>{reasons[reason] || eventAttention || feedError || (hasTerminal ? 'Agent needs input. A prompt may only be visible in the terminal.' : 'Agent needs input. Check the raw event log.')}</p>
    <div class="actions">
      {#if hasTerminal}<button type="button" class="lattice-btn lattice-btn--secondary" disabled={pending} onclick={flush}>{pending ? 'Flushing…' : 'Flush Enter'}</button>{/if}
      <button type="button" class="lattice-btn lattice-btn--ghost" onclick={openTerminal}>{hasTerminal ? 'Open terminal' : 'Open raw event log'}</button>
      {#if hasTerminal}
        <button type="button" class="lattice-btn lattice-btn--ghost" disabled title="original message unavailable">Resend</button>
        <span class="unavailable">original message unavailable</span>
      {/if}
    </div>
    {#if currentResult?.receipt !== undefined}<FlushReceipt body={currentResult.receipt} ondismiss={() => { result = null; }} />{/if}
    {#if currentResult?.error}<p role="alert">{currentResult.error}</p>{/if}
  </section>
{/if}

<style>
  .attention { padding: var(--space-3) var(--space-4); background: var(--bg-surface); box-shadow: var(--edge-seam); }
  .reason { margin: 0 0 var(--space-2); color: var(--text-primary); font-size: var(--text-small); }
  .actions { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); }
  .unavailable { color: var(--text-secondary); font-size: var(--text-small); }
</style>
