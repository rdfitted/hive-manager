<script lang="ts">
  import IconButton from '../ui/IconButton.svelte';
  import type { PaneMode } from './paneMode';

  let { hasTerminal, mode, onchange }: { hasTerminal: boolean; mode: PaneMode; onchange: (m: PaneMode) => void } = $props();

  function select(event: MouseEvent, next: PaneMode) {
    event.stopPropagation();
    onchange(next);
  }
</script>

<div class="pane-toggle lattice-forced-colors-boundary" role="group" aria-label="Agent pane">
  <IconButton type="button" label="Chat" tooltip="Chat" icon="chat" size="xs" pressed={mode === 'chat'} onclick={(event) => select(event, 'chat')} />
  {#if hasTerminal}
    <IconButton type="button" label="Split" tooltip="Chat with terminal below" icon="split" size="xs" pressed={mode === 'split'} onclick={(event) => select(event, 'split')} />
    <IconButton type="button" label="Terminal" tooltip="Terminal" icon="term" size="xs" pressed={mode === 'terminal'} onclick={(event) => select(event, 'terminal')} />
  {:else}
    <IconButton type="button" label="Raw event log" tooltip="Raw event log" icon="log" size="xs" pressed={mode === 'raw'} onclick={(event) => select(event, 'raw')} />
  {/if}
</div>

<style>
  /* Chat, Split and Terminal share one track; the pressed view takes the strong glass. */
  .pane-toggle { display: inline-flex; flex: none; padding: 2px; border-radius: var(--control-radius); background: var(--wash-strong); }
  .pane-toggle :global(.lattice-icon-button) { width: 32px; height: 22px; border-radius: 8px; color: var(--ink-dim); }
  .pane-toggle :global(.lattice-icon-button:hover:not(:disabled)) { background: transparent; color: var(--ink); }
  .pane-toggle :global(.lattice-icon-button[aria-pressed='true']) { border-color: transparent; background: var(--glass-strong); color: var(--ink); }
  @media (forced-colors: active) { .pane-toggle :global(.lattice-icon-button[aria-pressed='true']) { border-color: Highlight; } }
</style>
