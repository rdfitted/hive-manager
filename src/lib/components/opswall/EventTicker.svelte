<script lang="ts">
  import type { TickerItem } from './ticker';
  let { items, onselect }: { items: TickerItem[]; onselect: (id: string) => void } = $props();
</script>

<div class="event-ticker" role="region" aria-label="Session event ticker">
  <span class="ticker-label">Latest events</span>
  {#each items as item (item.id)}
    {#if item.agentId}<button type="button" class="ticker-item lattice-btn lattice-btn--ghost" title={item.at} onclick={() => onselect(item.agentId!)}>{item.text}</button>
    {:else}<span class="ticker-item" title={item.at}>{item.text}</span>{/if}
  {:else}<span class="ticker-empty">No session events yet</span>{/each}
</div>

<style>
  /* One quiet line; it scrolls sideways without a bar. */
  .event-ticker { display: flex; align-items: center; gap: 2px; min-width: 0; height: 20px; overflow-x: auto; overflow-y: hidden; font-size: 10.5px; line-height: 12px; }
  .event-ticker::-webkit-scrollbar { display: none; }
  .ticker-label { flex: 0 0 auto; padding-right: 6px; color: var(--ink-dim); }
  .event-ticker .ticker-item { flex: 0 0 auto; max-width: 24rem; min-height: 20px; padding: 0 8px; overflow: hidden; border-radius: 10px; color: var(--ink-muted); font-size: 10.5px; line-height: 20px; text-overflow: ellipsis; white-space: nowrap; }
  .event-ticker button.ticker-item { display: inline-block; }
  .event-ticker button.ticker-item:hover { background: var(--wash-strong); color: var(--ink); }
  .ticker-empty { padding: 0 8px; color: var(--ink-dim); }
</style>
