<script lang="ts">
  import { GridFour, Rectangle } from 'phosphor-svelte';
  import { layout } from '$lib/stores/layout';
  import { IconButton } from '../ui';

  const modes = [
    { id: 'grid', label: 'Grid', icon: GridFour },
    { id: 'focus', label: 'Focus', icon: Rectangle },
  ] as const;
</script>

<div class="view-mode-switch" role="group" aria-label="Terminal view">
  {#each modes as mode (mode.id)}
    <IconButton size="sm" icon={mode.icon} label={mode.label} tooltip={`${mode.label} view`}
      pressed={$layout.sessionViewMode === mode.id} onclick={() => layout.setSessionViewMode(mode.id)} />
  {/each}
</div>

<style>
  /* One segmented track, like the pane view switch. */
  .view-mode-switch {
    display: inline-flex;
    padding: 2px;
    border-radius: var(--control-radius);
    background: var(--wash-strong);
  }
  .view-mode-switch :global(.lattice-icon-button) {
    width: 30px;
    height: 24px;
    border-radius: 8px;
    color: var(--ink-dim);
  }
  .view-mode-switch :global(.lattice-icon-button[aria-pressed='true']) {
    border-color: transparent;
    color: var(--ink);
  }
  @media (forced-colors: active) {
    .view-mode-switch :global(.lattice-icon-button[aria-pressed='true']) { border-color: Highlight; }
  }
</style>
