<script lang="ts">
  import { Eye, Users } from 'phosphor-svelte';
  import { goto } from '$app/navigation';
  import { sessions, type Session } from '$lib/stores/sessions';

  interface Props {
    session: Session;
  }
  let { session }: Props = $props();

  let agentCount = $derived(session.agents?.length ?? 0);

  let lastActivity = $derived(session.created_at);

  let lastActivityLabel = $derived.by(() => {
    if (!lastActivity) return '—';
    const d = new Date(lastActivity);
    if (Number.isNaN(d.getTime())) return '—';
    const diffMs = Date.now() - d.getTime();
    const sec = Math.floor(diffMs / 1000);
    if (sec < 60) return `${sec}s ago`;
    const min = Math.floor(sec / 60);
    if (min < 60) return `${min}m ago`;
    const hr = Math.floor(min / 60);
    if (hr < 24) return `${hr}h ago`;
    return d.toLocaleDateString();
  });

  let title = $derived(session.name ?? session.id.slice(0, 8));

  function openSession() {
    sessions.setActiveSession(session.id);
    void goto('/');
  }
</script>

<article class="card" aria-label={title}>
  <header class="card-head">
    <h3 class="title" title={title}>{title}</h3>
    <button class="eye" aria-label="Open session" title="Open session" onclick={openSession}>
      <Eye size={16} weight="light" />
    </button>
  </header>
  <div class="meta">
    <span class="meta-item" title="Agents">
      <Users size={12} weight="light" />
      {agentCount}
    </span>
    <span class="meta-item" title={lastActivity ?? ''}>
      {lastActivityLabel}
    </span>
  </div>
</article>

<style>
  .card {
    background: var(--bg-elevated);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    padding: var(--space-3);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    transition: border-color var(--transition-fast);
  }
  .card:hover {
    border-color: var(--accent-cyan);
  }
  .card-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .title {
    margin: 0;
    font-family: var(--font-display);
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .eye {
    background: none;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    padding: var(--space-1);
    display: inline-flex;
    align-items: center;
    border-radius: var(--radius-sm);
  }
  .eye:hover {
    color: var(--accent-cyan);
    background: rgba(0, 229, 255, 0.08);
  }
  .meta {
    display: flex;
    gap: var(--space-3);
    font-size: var(--text-small);
    color: var(--text-secondary);
  }
  .meta-item {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
  }
</style>
