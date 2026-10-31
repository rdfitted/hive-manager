<script lang="ts">
  import { Eye, Users } from 'phosphor-svelte';
  import { goto } from '$app/navigation';
  import { sessions, type Session } from '$lib/stores/sessions';
  import type { CellStatus } from '$lib/types/domain';
  import { statusIconFor, statusIconWeight } from '$lib/components/cell/statusIcon';

  interface Props {
    session: Session;
    status: CellStatus;
  }
  let { session, status }: Props = $props();

  let agentCount = $derived(session.agents?.length ?? 0);
  let StatusIcon = $derived(statusIconFor(status));

  let lastActivity = $derived(session.last_activity_at ?? session.created_at);

  let lastActivityLabel = $derived.by(() => {
    if (!lastActivity) return '—';
    const d = new Date(lastActivity);
    if (Number.isNaN(d.getTime())) return '—';
    const diffMs = Date.now() - d.getTime();
    const sec = Math.max(0, Math.floor(diffMs / 1000));
    if (sec === 0) return 'just now';
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

<article class="card lattice-panel" aria-label={title} style:--session-color={session.color || 'var(--text-disabled)'}>
  <header class="card-head">
    <div class="title-wrap">
      <div class="status-icon" title={status} aria-hidden="true">
        <StatusIcon size={14} weight={statusIconWeight(status)} />
      </div>
      <h3 class="title" title={title}>{title}</h3>
    </div>
    <button type="button" class="lattice-btn lattice-btn--ghost lattice-btn--icon" aria-label="Open session" title="Open session" onclick={openSession}>
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
    background-color: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--control-radius);
    box-shadow: none;
    padding: var(--space-3);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    transition:
      background-color var(--motion-duration-fast) var(--motion-ease-standard),
      border-color var(--motion-duration-fast) var(--motion-ease-standard);
  }
  .card:hover {
    background-color: var(--surface-raised);
    border-color: var(--text-muted);
  }
  .card:focus-within {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }
  .card-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .title-wrap {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    flex: 1;
  }
  .status-icon {
    width: 24px;
    height: 24px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-sm);
    background: var(--canvas);
    box-shadow: inset 0 0 0 1px var(--session-color);
    color: var(--text-secondary);
    flex: 0 0 auto;
  }
  .title {
    margin: 0;
    font-family: var(--font-body);
    font-size: var(--text-small);
    font-weight: 600;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    flex: 1;
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
  @media (forced-colors: active) {
    .card { border-color: CanvasText; }
    .status-icon { box-shadow: none; border: 1px solid CanvasText; }
  }
</style>
