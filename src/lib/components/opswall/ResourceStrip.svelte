<script lang="ts">
  let { attention, waiting, stale, phase, filter, onfilter }: {
    attention: number; waiting: number; stale: number; phase: string;
    filter: string | null; onfilter: (filter: string | null) => void;
  } = $props();
</script>

<nav class="resource-strip" aria-label="Agent resources">
  {#each [{ id: 'attention', label: 'Attention', count: attention }, { id: 'waiting', label: 'Waiting', count: waiting }, { id: 'stale', label: 'Stale', count: stale }] as tile (tile.id)}
    <button type="button" class="resource-tile lattice-btn lattice-btn--ghost" aria-pressed={filter === tile.id} onclick={() => onfilter(filter === tile.id ? null : tile.id)}><span>{tile.label}</span><strong>{tile.count}</strong></button>
  {/each}
  <span class="phase"><span>Phase</span> <strong>{phase}</strong></span>
</nav>

<style>
  .resource-strip { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-1); min-width: 0; font-size: var(--text-small); }
  .resource-tile { display: inline-flex; gap: var(--space-2); }
  .resource-tile[aria-pressed='true'] { background: var(--surface-overlay); color: var(--text-primary); }
  .phase { display: inline-flex; flex-wrap: wrap; gap: var(--space-2); margin-left: auto; padding: var(--space-1) var(--space-2); color: var(--text-muted); }
  strong { color: var(--text-primary); }
</style>
