<script lang="ts">
  import { sessions as sessionsStore, sessionStateToCellStatus, type Session } from '$lib/stores/sessions';
  import type { CellStatus } from '$lib/types/domain';
  import KanbanColumn from './KanbanColumn.svelte';

  const PRIMARY_COLUMNS: { status: CellStatus; label: string; accent: string }[] = [
    { status: 'queued', label: 'Queued', accent: 'var(--status-queued)' },
    { status: 'preparing', label: 'Preparing', accent: 'var(--accent-chrome)' },
    { status: 'launching', label: 'Launching', accent: 'var(--accent-amber)' },
    { status: 'running', label: 'Running', accent: 'var(--status-running)' },
    { status: 'waiting_input', label: 'Waiting Input', accent: 'var(--status-warning)' },
    { status: 'summarizing', label: 'Summarizing', accent: 'var(--accent-chrome)' },
    { status: 'completed', label: 'Completed', accent: 'var(--status-success)' },
    { status: 'failed', label: 'Failed', accent: 'var(--status-error)' },
    { status: 'killed', label: 'Killed', accent: 'var(--status-blocked)' },
  ];

  let groups = $derived.by(() => {
    const map = new Map<CellStatus, Session[]>();
    for (const s of $sessionsStore.sessions) {
      const k = sessionStateToCellStatus(s.state);
      const arr = map.get(k) ?? [];
      arr.push(s);
      map.set(k, arr);
    }
    return map;
  });
</script>

<div class="board">
  <div class="columns">
    {#each PRIMARY_COLUMNS as col (col.status)}
      <KanbanColumn
        label={col.label}
        status={col.status}
        accent={col.accent}
        sessions={groups.get(col.status) ?? []}
      />
    {/each}
  </div>
</div>

<style>
  .board {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    height: 100%;
    min-height: 0;
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
