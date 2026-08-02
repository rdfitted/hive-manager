<script lang="ts">
  import { onMount } from 'svelte';
  import KanbanBoard from '$lib/components/dashboard/KanbanBoard.svelte';
  import { sessions } from '$lib/stores/sessions';

  onMount(() => {
    let pollInFlight = false;
    const poll = async () => {
      if (pollInFlight) return;
      pollInFlight = true;
      try {
        await sessions.loadSessions();
      } finally {
        pollInFlight = false;
      }
    };

    poll();
    const intervalId = window.setInterval(poll, 5000);

    return () => {
      window.clearInterval(intervalId);
    };
  });
</script>

<div class="dashboard">
  <header class="page-head">
    <div>
      <h1>Dashboard</h1>
      <p class="subtitle">All sessions grouped by status</p>
    </div>
  </header>
  <div class="board-wrap">
    <KanbanBoard />
  </div>
</div>

<style>
  .dashboard {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    height: 100%;
    background: var(--color-bg);
    color: var(--color-text);
    padding: var(--space-5);
    gap: var(--space-4);
    font-family: var(--font-body);
    overflow: hidden;
  }
  .page-head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    box-shadow: var(--edge-seam);
    padding-bottom: var(--space-3);
  }
  h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: var(--text-h1);
    font-weight: 700;
    letter-spacing: 0.04em;
  }
  .subtitle {
    margin: var(--space-1) 0 0 0;
    color: var(--text-secondary);
    font-size: var(--text-small);
  }
  .board-wrap {
    flex: 1;
    min-height: 0;
  }
</style>
