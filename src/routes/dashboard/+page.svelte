<script lang="ts">
  import { onMount } from 'svelte';
  import { House } from 'phosphor-svelte';
  import KanbanBoard from '$lib/components/dashboard/KanbanBoard.svelte';
  import { sessions } from '$lib/stores/sessions';

  onMount(() => {
    sessions.loadSessions();
  });
</script>

<div class="dashboard">
  <header class="page-head">
    <div>
      <h1>Dashboard</h1>
      <p class="subtitle">All sessions grouped by status</p>
    </div>
    <a href="/" class="home-link" title="Back to session view">
      <House size={16} weight="light" />
      Session View
    </a>
  </header>
  <div class="board-wrap">
    <KanbanBoard />
  </div>
</div>

<style>
  .dashboard {
    display: flex;
    flex-direction: column;
    width: 100vw;
    height: 100vh;
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
    border-bottom: 1px solid var(--color-border);
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
  .home-link {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--text-secondary);
    text-decoration: none;
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    font-size: var(--text-small);
  }
  .home-link:hover {
    color: var(--accent-cyan);
    border-color: var(--accent-cyan);
  }
  .board-wrap {
    flex: 1;
    min-height: 0;
  }
  :global(body) {
    margin: 0;
    padding: 0;
  }
</style>
