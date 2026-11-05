<script lang="ts">
  import { sessions as sessionsStore, sessionStateToCellStatus, type Session } from '$lib/stores/sessions';
  import type { CellStatus } from '$lib/types/domain';
  import KanbanColumn from './KanbanColumn.svelte';

  const PRIMARY_COLUMNS: { status: CellStatus; label: string; accent: string }[] = [
    { status: 'preparing', label: 'Preparing', accent: 'var(--accent-chrome)' },
    { status: 'running', label: 'Running', accent: 'var(--status-running)' },
    { status: 'waiting_input', label: 'Waiting Input', accent: 'var(--status-warning)' },
    { status: 'completed', label: 'Completed', accent: 'var(--status-success)' },
    { status: 'failed', label: 'Failed', accent: 'var(--status-error)' },
  ];

  function boardStatus(status: CellStatus): CellStatus | 'other' {
    if (status === 'launching') return 'running';
    if (status === 'summarizing') return 'completed';
    return PRIMARY_COLUMNS.some((column) => column.status === status) ? status : 'other';
  }

  let groups = $derived.by(() => {
    const map = new Map<CellStatus | 'other', Session[]>();
    for (const s of $sessionsStore.sessions) {
      const k = boardStatus(sessionStateToCellStatus(s.state));
      const arr = map.get(k) ?? [];
      arr.push(s);
      map.set(k, arr);
    }
    return map;
  });
</script>

<div class="board">
  <div class="columns lattice-scroll-content">
    {#each PRIMARY_COLUMNS as col (col.status)}
      <KanbanColumn
        label={col.label}
        status={col.status}
        accent={col.accent}
        sessions={groups.get(col.status) ?? []}
      />
    {/each}
    {#if (groups.get('other')?.length ?? 0) > 0}
      <KanbanColumn
        label="Other"
        status="queued"
        accent="var(--text-muted)"
        sessions={groups.get('other') ?? []}
      />
    {/if}
  </div>
</div>

<style>
  .board {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    height: 100%;
    min-height: 0;
    background: var(--canvas);
  }
  .columns {
    display: flex;
    gap: var(--space-3);
    overflow-x: auto;
    padding-bottom: var(--space-2);
    flex: 1;
    min-height: 0;
  }
</style>
