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

<section class="column" style="--col-accent: {accent};" aria-label={label}>
  <header class="col-head">
    <span class="dot" aria-hidden="true"></span>
    <span class="label">{label}</span>
    <span class="count">{items.length}</span>
  </header>
  <div class="col-body">
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
    min-width: 220px;
    width: 220px;
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    max-height: 100%;
  }
  .col-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-3);
    border-bottom: 1px solid var(--color-border);
    font-family: var(--font-display);
    font-size: var(--text-small);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-primary);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--col-accent);
    box-shadow: 0 0 8px var(--col-accent);
  }
  .label {
    flex: 1;
  }
  .count {
    color: var(--text-secondary);
    font-family: var(--font-mono);
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
</style>
