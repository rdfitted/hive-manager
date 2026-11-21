<script lang="ts">
  import type { Snippet } from 'svelte';
  import { Hourglass, Timer } from 'phosphor-svelte';
  import Glyph from '../ui/Glyph.svelte';

  type Metrics = { agents: number; running: number; elapsed: string | null; nodes: string | null; files: number | null };
  let { attention, waiting, stale, phase, filter, onfilter, metrics, children }: {
    attention: number; waiting: number; stale: number; phase: string;
    metrics?: Metrics;
    /** Rendered between the counts, e.g. the event ticker in the band. */
    children?: Snippet;
    filter: string | null; onfilter: (filter: string | null) => void;
  } = $props();
</script>

{#snippet metricValue(label: string, value: string | number | null)}
  <span class="metric-label">{label}{' '}{value === null ? 'unavailable' : ''}</span>
  <span aria-hidden={value === null ? 'true' : undefined}>{value ?? '—'}</span>
{/snippet}

<nav class="resource-strip" aria-label="Agent resources">
  {#if metrics}
    <span class="metric" title={`${metrics.agents} agents`}><Glyph name="hex" size={12} />{@render metricValue('Agents', metrics.agents)}</span>
    <span class="metric" title={`${metrics.running} running`}><Glyph name="activity" size={12} />{@render metricValue('Running', metrics.running)}</span>
  {/if}
  {#each [
    { id: 'attention', label: 'Attention', count: attention, icon: null, tip: 'Need your attention' },
    { id: 'waiting', label: 'Waiting', count: waiting, icon: Hourglass, tip: 'Waiting for input' },
    { id: 'stale', label: 'Stale', count: stale, icon: Timer, tip: 'Stale activity' },
  ] as tile (tile.id)}
    <button type="button" class="resource-tile lattice-btn lattice-btn--ghost"
      class:hot={tile.id === 'attention' && tile.count > 0}
      aria-label={`${tile.label} ${tile.count}`} title={tile.tip}
      aria-pressed={filter === tile.id} onclick={() => onfilter(filter === tile.id ? null : tile.id)}>
      {#if tile.icon}<tile.icon size={12} aria-hidden="true" />{:else}<Glyph name="bell" size={12} />{/if}<strong>{tile.count}</strong>
    </button>
  {/each}
  {#if children}<div class="strip-middle">{@render children()}</div>{/if}
  {#if metrics}
    <div class="band-metrics">
      <span class="metric" title="Elapsed time"><Glyph name="clock" size={12} />{@render metricValue('Elapsed', metrics.elapsed)}</span>
      <span class="metric" title="Completed work graph nodes"><Glyph name="check" size={12} />{@render metricValue('Nodes', metrics.nodes)}</span>
      <span class="metric" title="Changed files reported in artifacts"><Glyph name="file" size={12} />{@render metricValue('Files', metrics.files)}</span>
    </div>
  {/if}
  <span class="phase" class:lattice-sr-only={metrics !== undefined}><span>Phase</span> <strong>{phase}</strong></span>
</nav>

<style>
  /* Counts as icons with tooltips; the one hot count takes the accent. */
  .resource-strip { display: flex; align-items: center; flex-wrap: wrap; gap: 2px 10px; min-width: 0; color: var(--ink-muted); font-size: 10.5px; line-height: 12px; font-variant-numeric: tabular-nums; }
  .resource-tile, .metric { display: inline-flex; align-items: center; gap: 5px; }
  .resource-strip .resource-tile { min-height: 20px; padding: 0 6px; margin-inline: -6px 0; border-radius: 10px; color: var(--ink-muted); font-size: 10.5px; }
  .resource-strip .resource-tile:hover:enabled { background: var(--wash-strong); color: var(--ink); }
  .resource-strip .resource-tile.hot { color: var(--accent); font-weight: 600; }
  .resource-strip .resource-tile[aria-pressed='true'] { background: var(--glass-strong); box-shadow: inset 0 0 0 1px var(--line); color: var(--ink); }
  .resource-strip .resource-tile.hot[aria-pressed='true'] { color: var(--accent); }
  .metric { color: var(--ink-muted); }
  .strip-middle { flex: 1 1 16rem; min-width: 0; }
  .band-metrics { display: flex; gap: 12px; margin-left: auto; }
  .phase.lattice-sr-only, .metric-label { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
  .phase { display: inline-flex; flex-wrap: wrap; gap: var(--space-2); margin-left: auto; padding: var(--space-1) var(--space-2); color: var(--ink-muted); }
  strong { color: inherit; font-weight: inherit; }
  .phase strong { color: var(--ink); font-weight: 600; }
  @media (forced-colors: active) {
    .resource-tile[aria-pressed='true'] { outline: 1px solid Highlight; }
  }
</style>
