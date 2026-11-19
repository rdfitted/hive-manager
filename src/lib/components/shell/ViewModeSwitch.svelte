<script lang="ts">
  import { GridFour, Rectangle } from 'phosphor-svelte';
  import { layout } from '$lib/stores/layout';
  import { IconButton } from '../ui';

  const modes = [
    { id: 'focus', label: 'Focus', icon: Rectangle },
    { id: 'grid', label: 'Grid', icon: GridFour },
  ] as const;
</script>

<div class="view-mode-switch" role="group" aria-label="Terminal view">
  {#each modes as mode (mode.id)}
    <IconButton size="sm" icon={mode.icon} label={mode.label} tooltip={`${mode.label} view`}
      pressed={$layout.sessionViewMode === mode.id} onclick={() => layout.setSessionViewMode(mode.id)} />
  {/each}
</div>

<style>
  .view-mode-switch {
    display: flex;
    gap: var(--space-1);
  }
</style>
