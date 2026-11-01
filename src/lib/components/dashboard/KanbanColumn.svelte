<script lang="ts">
  import type { Session } from '$lib/stores/sessions';
  import type { CellStatus } from '$lib/types/domain';
  import KanbanCard from './KanbanCard.svelte';

  interface Props {
    label: string;
    sessions: Session[];
    status?: CellStatus;
    accent?: string;
  }
  let { label, sessions: items, status = 'queued', accent = 'var(--text-secondary)' }: Props = $props();
</script>

<section class="column lattice-forced-colors-boundary" style="--col-accent: {accent};" aria-label={label}>
  <header class="col-head">
    <span class="dot" aria-hidden="true"></span>
    <span class="label">{label}</span>
    <span class="count">{items.length}</span>
  </header>
  <div class="col-body lattice-scroll-content">
    {#each items as s (s.id)}
      <KanbanCard session={s} status={status} />
    {/each}
    {#if items.length === 0}
      <div class="empty">—</div>
    {/if}
  </div>
</section>

<style>
  .column {
    display: flex;
    flex-direction: column;
    min-width: 240px;
    width: 240px;
    max-height: 100%;
    overflow: hidden;
    background: var(--chrome);
    border: 1px solid var(--border);
    border-radius: var(--control-radius);
  }
  .col-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: var(--toolbar-height);
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
    font-family: var(--font-body);
    font-size: var(--text-small);
    font-weight: 600;
    color: var(--text-primary);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--col-accent);
  }
  .label {
    flex: 1;
  }
  .count {
    color: var(--text-muted);
    font-family: var(--font-body);
    font-variant-numeric: tabular-nums;
    padding: 1px 6px;
    border-radius: var(--radius-sm);
    background: var(--surface);
  }
  .col-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
    overflow-y: auto;
    flex: 1;
  }
  .empty {
    color: var(--text-disabled);
    font-size: var(--text-small);
    text-align: center;
    padding: var(--space-3) 0;
  }
  @media (forced-colors: active) {
    .column, .col-head { border-color: CanvasText; }
    .dot { forced-color-adjust: none; background: CanvasText; }
  }
</style>
