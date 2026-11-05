<script lang="ts">
  import type { TickerItem } from './ticker';
  let { items, onselect }: { items: TickerItem[]; onselect: (id: string) => void } = $props();
</script>

<div class="event-ticker lattice-scroll-content" role="region" aria-label="Session event ticker">
  <span class="ticker-label" aria-hidden="true">Latest events</span>
  {#each items as item (item.id)}
    {#if item.agentId}<button type="button" class="ticker-item lattice-btn lattice-btn--ghost" title={item.at} onclick={() => onselect(item.agentId!)}>{item.text}</button>
    {:else}<span class="ticker-item" title={item.at}>{item.text}</span>{/if}
  {:else}<span class="ticker-empty">No session events yet</span>{/each}
</div>

<style>
  .event-ticker { display: flex; gap: var(--space-3); overflow-x: auto; min-width: 0; max-height: 38px; align-items: center; font-size: var(--text-micro); }
  .ticker-label { flex: 0 0 auto; align-self: center; color: var(--text-muted); }
  .ticker-item { flex: 0 0 auto; max-width: 28rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; padding: var(--space-1) var(--space-2); }
  .ticker-empty { padding: var(--space-1) var(--space-2); color: var(--text-muted); }
</style>
