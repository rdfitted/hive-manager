<script lang="ts">
  import { CaretDown, CaretRight } from 'phosphor-svelte';
  import { sessions as sessionsStore, type Session, type SessionState } from '$lib/stores/sessions';
  import type { CellStatus } from '$lib/types/domain';
  import KanbanColumn from './KanbanColumn.svelte';

  function stateKey(state: SessionState): string {
    if (typeof state === 'string') return state;
    return Object.keys(state)[0] ?? 'Unknown';
  }

  function sessionToCellStatus(session: Session): CellStatus {
    const key = stateKey(session.state);
    switch (key) {
      case 'Planning':
      case 'PlanReady':
        return 'preparing';
      case 'Starting':
        return 'launching';
      case 'Running':
      case 'SpawningEvaluator':
      case 'QaInProgress':
        return 'running';
      case 'QaPassed':
        return 'summarizing';
      case 'Paused':
      case 'QaFailed':
        return 'waiting_input';
      case 'QaMaxRetriesExceeded':
      case 'Failed':
        return 'failed';
      case 'Completed':
      case 'Closed':
        return 'completed';
      default:
        return 'queued';
    }
  }

  const PRIMARY_COLUMNS: { status: CellStatus; label: string; accent: string }[] = [
    { status: 'queued', label: 'Queued', accent: 'var(--status-queued)' },
    { status: 'preparing', label: 'Preparing', accent: 'var(--accent-chrome)' },
    { status: 'launching', label: 'Launching', accent: 'var(--accent-amber)' },
    { status: 'running', label: 'Running', accent: 'var(--status-running)' },
    { status: 'waiting_input', label: 'Waiting Input', accent: 'var(--status-warning)' },
    { status: 'summarizing', label: 'Summarizing', accent: 'var(--accent-chrome)' },
    { status: 'completed', label: 'Completed', accent: 'var(--status-success)' },
  ];

  let collapsedFailed = $state(true);

  let groups = $derived.by(() => {
    const map = new Map<CellStatus, Session[]>();
    for (const s of $sessionsStore.sessions) {
      const k = sessionToCellStatus(s);
      const arr = map.get(k) ?? [];
      arr.push(s);
      map.set(k, arr);
    }
    return map;
  });

  let failedKilled = $derived([...(groups.get('failed') ?? []), ...(groups.get('killed') ?? [])]);
</script>

<div class="board">
  <div class="columns">
    {#each PRIMARY_COLUMNS as col (col.status)}
      <KanbanColumn
        label={col.label}
        accent={col.accent}
        sessions={groups.get(col.status) ?? []}
      />
    {/each}
  </div>

  <section class="failed-section" class:collapsed={collapsedFailed}>
    <button
      class="failed-header"
      aria-expanded={!collapsedFailed}
      onclick={() => (collapsedFailed = !collapsedFailed)}
    >
      {#if collapsedFailed}
        <CaretRight size={14} weight="bold" />
      {:else}
        <CaretDown size={14} weight="bold" />
      {/if}
      <span>Failed / Killed</span>
      <span class="count">{failedKilled.length}</span>
    </button>
    {#if !collapsedFailed}
      <div class="columns">
        <KanbanColumn
          label="Failed"
          accent="var(--status-error)"
          sessions={groups.get('failed') ?? []}
        />
        <KanbanColumn
          label="Killed"
          accent="var(--status-blocked)"
          sessions={groups.get('killed') ?? []}
        />
      </div>
    {/if}
  </section>
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
  .failed-section {
    border-top: 1px solid var(--color-border);
    padding-top: var(--space-3);
  }
  .failed-header {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    background: none;
    border: none;
    cursor: pointer;
    color: var(--text-primary);
    font-family: var(--font-display);
    font-size: var(--text-small);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    padding: var(--space-2) 0;
  }
  .failed-header:hover {
    color: var(--status-error);
  }
  .count {
    color: var(--text-secondary);
    font-family: var(--font-mono);
  }
</style>
