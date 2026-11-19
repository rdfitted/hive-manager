<script lang="ts">
  import { Bell, Clock, File, Graph, Hourglass, Pulse, Timer, Users } from 'phosphor-svelte';

  type Metrics = { agents: number; running: number; elapsed: string | null; nodes: string | null; files: number | null };
  let { attention, waiting, stale, phase, filter, onfilter, metrics }: {
    attention: number; waiting: number; stale: number; phase: string;
    metrics?: Metrics;
    filter: string | null; onfilter: (filter: string | null) => void;
  } = $props();
</script>

<nav class="resource-strip" aria-label="Agent resources">
  {#if metrics}
    <span class="metric" title={`${metrics.agents} agents`}><Users size={14} aria-hidden="true" /><span aria-label={`Agents ${metrics.agents}`}>{metrics.agents}</span></span>
    <span class="metric" title={`${metrics.running} running`}><Pulse size={14} aria-hidden="true" /><span aria-label={`Running ${metrics.running}`}>{metrics.running}</span></span>
  {/if}
  {#each [
    { id: 'attention', label: 'Attention', count: attention, icon: Bell, tip: 'Need your attention' },
    { id: 'waiting', label: 'Waiting', count: waiting, icon: Hourglass, tip: 'Waiting for input' },
    { id: 'stale', label: 'Stale', count: stale, icon: Timer, tip: 'Stale activity' },
  ] as tile (tile.id)}
    <button type="button" class="resource-tile lattice-btn lattice-btn--ghost"
      aria-label={`${tile.label} ${tile.count}`} title={tile.tip}
      aria-pressed={filter === tile.id} onclick={() => onfilter(filter === tile.id ? null : tile.id)}>
      <tile.icon size={14} aria-hidden="true" /><strong>{tile.count}</strong>
    </button>
  {/each}
  {#if metrics}
    <div class="band-metrics">
      <span class="metric" title="Elapsed time"><Clock size={14} aria-hidden="true" /><span aria-label={`Elapsed ${metrics.elapsed ?? 'unavailable'}`}>{metrics.elapsed ?? '—'}</span></span>
      <span class="metric" title="Completed work graph nodes"><Graph size={14} aria-hidden="true" /><span aria-label={`Nodes ${metrics.nodes ?? 'unavailable'}`}>{metrics.nodes ?? '—'}</span></span>
      <span class="metric" title="Changed files reported in artifacts"><File size={14} aria-hidden="true" /><span aria-label={`Files ${metrics.files ?? 'unavailable'}`}>{metrics.files ?? '—'}</span></span>
    </div>
  {/if}
  <span class="phase" class:lattice-sr-only={metrics !== undefined}><span>Phase</span> <strong>{phase}</strong></span>
</nav>

<style>
  .resource-strip { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-1); min-width: 0; font-size: var(--text-small); }
  .resource-tile, .metric { display: inline-flex; align-items: center; gap: var(--space-1); }
  .resource-tile { min-height: var(--control-height-sm); padding: 0 var(--space-2); }
  .metric { padding-inline: var(--space-1); color: var(--ink-muted); font-variant-numeric: tabular-nums; }
  .band-metrics { display: flex; gap: var(--space-2); margin-left: auto; }
  .phase.lattice-sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
  @media (forced-colors: active) {
    .resource-tile[aria-pressed='true'] { outline: 1px solid Highlight; }
  }
  .resource-tile[aria-pressed='true'] { background: var(--surface-overlay); color: var(--text-primary); }
  .phase { display: inline-flex; flex-wrap: wrap; gap: var(--space-2); margin-left: auto; padding: var(--space-1) var(--space-2); color: var(--text-muted); }
  strong { color: var(--text-primary); }
</style>
