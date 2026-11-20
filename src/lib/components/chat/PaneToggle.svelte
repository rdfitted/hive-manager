<script lang="ts">
  import { ChatCircle, ListBullets, SquareSplitVertical, TerminalWindow } from 'phosphor-svelte';
  import IconButton from '../ui/IconButton.svelte';
  import type { PaneMode } from './paneMode';

  let { hasTerminal, mode, onchange }: { hasTerminal: boolean; mode: PaneMode; onchange: (m: PaneMode) => void } = $props();

  function select(event: MouseEvent, next: PaneMode) {
    event.stopPropagation();
    onchange(next);
  }
</script>

<div class="pane-toggle lattice-forced-colors-boundary" role="group" aria-label="Agent pane">
  <IconButton type="button" label="Chat" tooltip="Chat" icon={ChatCircle} size="sm" pressed={mode === 'chat'} onclick={(event) => select(event, 'chat')} />
  {#if hasTerminal}
    <IconButton type="button" label="Split" tooltip="Chat with terminal below" icon={SquareSplitVertical} size="sm" pressed={mode === 'split'} onclick={(event) => select(event, 'split')} />
    <IconButton type="button" label="Terminal" tooltip="Terminal" icon={TerminalWindow} size="sm" pressed={mode === 'terminal'} onclick={(event) => select(event, 'terminal')} />
  {:else}
    <IconButton type="button" label="Raw event log" tooltip="Raw event log" icon={ListBullets} size="sm" pressed={mode === 'raw'} onclick={(event) => select(event, 'raw')} />
  {/if}
</div>

<style>
  .pane-toggle { display: flex; flex-wrap: wrap; gap: var(--space-1); padding: var(--space-1); border-radius: var(--radius-full); background: var(--track); }
</style>
