<script lang="ts">
  import { layout } from '$lib/stores/layout';
  const modes = ['focus', 'grid'] as const;
</script>

<div class="view-mode-switch lattice-track" role="group" aria-label="Terminal view">
  {#each modes as mode}
    <button
      type="button"
      class="lattice-tab"
      aria-pressed={$layout.sessionViewMode === mode}
      onclick={() => layout.setSessionViewMode(mode)}
    >{mode === 'focus' ? 'Focus' : 'Grid'}</button>
  {/each}
</div>

<style>
  .view-mode-switch {
    display: flex;
    gap: 2px;
  }
</style>
