<script lang="ts">
  let { hasTerminal, mode, onchange }: { hasTerminal: boolean; mode: 'chat' | 'terminal' | 'raw'; onchange: (mode: 'chat' | 'terminal' | 'raw') => void } = $props();
</script>

<div class="pane-toggle lattice-forced-colors-boundary" role="group" aria-label="Agent pane">
  <button type="button" class="lattice-btn lattice-btn--ghost" aria-pressed={mode === 'chat'} onclick={(event) => { event.stopPropagation(); onchange('chat'); }}>Chat</button>
  {#if hasTerminal}
    <button type="button" class="lattice-btn lattice-btn--ghost" aria-pressed={mode === 'terminal'} onclick={(event) => { event.stopPropagation(); onchange('terminal'); }}>Terminal</button>
  {:else}
    <button type="button" class="lattice-btn lattice-btn--ghost" aria-pressed={mode === 'raw'} onclick={(event) => { event.stopPropagation(); onchange('raw'); }}>Raw event log</button>
  {/if}
</div>

<style>
  .pane-toggle { display: flex; flex-wrap: wrap; gap: var(--space-1); }
  .pane-toggle button { padding: var(--space-1) var(--space-2); font-size: var(--text-small); }
  .pane-toggle button[aria-pressed='true'] { color: var(--text-primary); box-shadow: inset 0 0 0 1px var(--border-control); }
</style>
