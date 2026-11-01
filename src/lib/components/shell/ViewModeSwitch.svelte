<script lang="ts">
  import { layout } from '$lib/stores/layout';
  const modes = ['focus', 'grid'] as const;
</script>

<div class="view-mode-switch" role="group" aria-label="Terminal view">
  {#each modes as mode}
    <button
      type="button"
      class={`lattice-btn lattice-btn--compact ${$layout.sessionViewMode === mode ? 'lattice-btn--secondary' : 'lattice-btn--ghost'}`}
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
