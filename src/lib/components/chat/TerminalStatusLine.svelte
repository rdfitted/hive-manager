<script lang="ts">
  import { activeSession } from '$lib/stores/sessions';
  import AgentActivityLine from '../agent/AgentActivityLine.svelte';
  import Glyph from '../ui/Glyph.svelte';
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
  {#if !compact}<span class="status-glyph" title="Live terminal output"><Glyph name="term" size={13} /></span>{/if}
  {#if line}
    <span class="terminal-line" title={line}>{line}</span>
  {:else if $activeSession?.id === sessionId}
    <AgentActivityLine {agentId} {compact} />
  {:else}
    <span class="terminal-line">No activity yet</span>
  {/if}
  {#if onOpenSplit}
    <IconButton type="button" label="Open Split" tooltip="Show terminal" icon="up" size="xs" onclick={(event) => {
      event.stopPropagation();
      onOpenSplit?.();
    }} />
  {/if}
</div>

<style>
  /* The live terminal, whispered in one line above the composer. */
  .terminal-status-line {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
    height: 28px;
    padding: 0 8px 0 16px;
    border-top: 1px solid var(--line);
    color: var(--ink-dim);
    font: 11px/1 var(--font-mono);
    white-space: nowrap;
  }
  .terminal-status-line.compact { height: auto; padding: 0; border-top: 0; font-size: 11.5px; }
  .status-glyph { display: inline-flex; flex: none; }
  .terminal-line { flex: 1; min-width: 0; overflow: hidden; color: var(--ink-muted); text-overflow: ellipsis; white-space: nowrap; }
  .terminal-status-line :global(.activity-line) { flex: 1; font-family: var(--font-mono); font-size: inherit; }
  .terminal-status-line :global(.lattice-icon-button) { margin-left: auto; color: var(--accent); }
  @media (forced-colors: active) { .terminal-status-line { border-color: CanvasText; color: CanvasText; } }
</style>
