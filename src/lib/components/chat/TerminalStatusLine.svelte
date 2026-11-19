<script lang="ts">
  import { ArrowUpRight } from 'phosphor-svelte';
  import { activeSession } from '$lib/stores/sessions';
  import AgentActivityLine from '../agent/AgentActivityLine.svelte';
  import IconButton from '../ui/IconButton.svelte';

  let { sessionId, agentId, line, onOpenSplit, compact = false }: {
    sessionId: string;
    agentId: string;
    line?: string;
    onOpenSplit?: () => void;
    compact?: boolean;
  } = $props();
</script>

<div class="terminal-status-line" class:compact>
  {#if line}
    <span class="terminal-line" title={line}>{line}</span>
  {:else if $activeSession?.id === sessionId}
    <AgentActivityLine {agentId} {compact} />
  {:else}
    <span class="terminal-line">No activity yet</span>
  {/if}
  {#if onOpenSplit}
    <IconButton type="button" label="Open Split" tooltip="Open Split" icon={ArrowUpRight} size="sm" onclick={(event) => {
      event.stopPropagation();
      onOpenSplit?.();
    }} />
  {/if}
</div>

<style>
  .terminal-status-line {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    padding: var(--space-1) 0;
    color: var(--ink-muted);
    font: var(--text-small) var(--font-mono);
  }
  .terminal-status-line.compact { padding: 0; }
  .terminal-line { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  @media (forced-colors: active) { .terminal-status-line { color: CanvasText; } }
</style>
