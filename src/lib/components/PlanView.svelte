<script lang="ts">
  import { activeSession } from '$lib/stores/sessions';
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';

  interface PlanTask {
    id: string;
    title: string;
    description: string;
    status: 'pending' | 'in_progress' | 'completed' | 'blocked';
    assignee?: string;
    priority?: 'high' | 'medium' | 'low';
  }

  interface Plan {
    title: string;
    summary: string;
    tasks: PlanTask[];
    generatedAt: string;
  }

  let plan: Plan | null = $state(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let lastSessionId: string | null = null;

  // Load plan when session changes
  $effect(() => {
    const sessionId = $activeSession?.id;
    if (sessionId && sessionId !== lastSessionId) {
      lastSessionId = sessionId;
      loadPlan(sessionId);
    } else if (!sessionId) {
      plan = null;
      lastSessionId = null;
    }
  });

  async function loadPlan(sessionId: string) {
    loading = true;
    error = null;

    try {
      // Try to load plan.md from the session directory
      const planData = await invoke<Plan | null>('get_session_plan', { sessionId });
      plan = planData;
    } catch (e) {
      // Plan might not exist yet - that's okay
      plan = null;
      console.log('No plan available:', e);
    } finally {
      loading = false;
    }
  }

  function getStatusIcon(status: PlanTask['status']): string {
    switch (status) {
      case 'completed': return '✓';
      case 'in_progress': return '●';
      case 'blocked': return '!';
      default: return '○';
    }
  }

  function getStatusColor(status: PlanTask['status']): string {
    switch (status) {
      case 'completed': return 'var(--color-success, #9ece6a)';
      case 'in_progress': return 'var(--color-running, #7aa2f7)';
      case 'blocked': return 'var(--color-error, #f7768e)';
      default: return 'var(--text-secondary, #565f89)';
    }
  }

  function getPriorityBadge(priority?: PlanTask['priority']): string {
    switch (priority) {
      case 'high': return 'H';
      case 'medium': return 'M';
      case 'low': return 'L';
      default: return '';
    }
  }

  function getPriorityColor(priority?: PlanTask['priority']): string {
    switch (priority) {
      case 'high': return 'var(--color-error, #f7768e)';
      case 'medium': return 'var(--color-warning, #e0af68)';
      case 'low': return 'var(--text-secondary, #565f89)';
      default: return 'transparent';
    }
  }
</script>

<div class="plan-view">
  {#if loading}
    <div class="loading">
      <span class="spinner">◐</span>
      Loading plan...
    </div>
  {:else if !$activeSession}
    <div class="empty-state">
      <span class="icon">📋</span>
      <p>No active session</p>
    </div>
  {:else if !plan}
    <div class="empty-state">
      <span class="icon">📝</span>
      <p>No plan generated yet</p>
      <span class="hint">The Master Planner will create a plan when the session starts.</span>
    </div>
  {:else}
    <div class="plan-header">
      <h3>{plan.title}</h3>
      <p class="summary">{plan.summary}</p>
      <span class="timestamp">Generated: {new Date(plan.generatedAt).toLocaleString()}</span>
    </div>

    <div class="tasks-header">
      <span class="tasks-title">Tasks</span>
      <span class="tasks-count">{plan.tasks.filter(t => t.status === 'completed').length}/{plan.tasks.length}</span>
    </div>

    <div class="tasks-list">
      {#each plan.tasks as task (task.id)}
        <div class="task-item" class:completed={task.status === 'completed'}>
          <span class="task-status" style="color: {getStatusColor(task.status)}">
            {getStatusIcon(task.status)}
          </span>
          <div class="task-content">
            <div class="task-header">
              <span class="task-title">{task.title}</span>
              {#if task.priority}
                <span class="priority-badge" style="background: {getPriorityColor(task.priority)}">
                  {getPriorityBadge(task.priority)}
                </span>
              {/if}
            </div>
            {#if task.description}
              <p class="task-description">{task.description}</p>
            {/if}
            {#if task.assignee}
              <span class="task-assignee">→ {task.assignee}</span>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}

  {#if error}
    <div class="error">{error}</div>
  {/if}
</div>

<style>
  .plan-view {
    display: flex;
    flex-direction: column;
    height: 100%;
    padding: 16px;
    overflow-y: auto;
  }

  .loading {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 32px;
    color: var(--text-secondary, #565f89);
  }

  .spinner {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 48px 24px;
    text-align: center;
  }

  .empty-state .icon {
    font-size: 48px;
    margin-bottom: 16px;
    opacity: 0.5;
  }

  .empty-state p {
    margin: 0;
    color: var(--text-secondary, #565f89);
    font-size: 14px;
  }

  .empty-state .hint {
    margin-top: 8px;
    color: var(--text-muted, #3b4261);
    font-size: 12px;
  }

  .plan-header {
    margin-bottom: 20px;
    padding-bottom: 16px;
    border-bottom: 1px solid var(--border-color, #414868);
  }

  .plan-header h3 {
    margin: 0 0 8px 0;
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary, #c0caf5);
  }

  .plan-header .summary {
    margin: 0 0 8px 0;
    font-size: 13px;
    color: var(--text-secondary, #a9b1d6);
    line-height: 1.5;
  }

  .plan-header .timestamp {
    font-size: 11px;
    color: var(--text-muted, #3b4261);
  }

  .tasks-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 12px;
  }

  .tasks-title {
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--text-secondary, #565f89);
  }

  .tasks-count {
    font-size: 12px;
    color: var(--text-muted, #3b4261);
  }

  .tasks-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .task-item {
    display: flex;
    gap: 10px;
    padding: 12px;
    background: var(--bg-tertiary, #24283b);
    border-radius: 6px;
    transition: opacity 0.15s;
  }

  .task-item.completed {
    opacity: 0.6;
  }

  .task-status {
    font-size: 14px;
    flex-shrink: 0;
    width: 20px;
    text-align: center;
  }

  .task-content {
    flex: 1;
    min-width: 0;
  }

  .task-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }

  .task-title {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary, #c0caf5);
  }

  .priority-badge {
    font-size: 10px;
    font-weight: 600;
    padding: 1px 5px;
    border-radius: 3px;
    color: white;
  }

  .task-description {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary, #a9b1d6);
    line-height: 1.4;
  }

  .task-assignee {
    display: inline-block;
    margin-top: 6px;
    font-size: 11px;
    color: var(--accent-color, #7aa2f7);
  }

  .error {
    padding: 12px;
    background: var(--error-bg, #3b2030);
    color: var(--error-text, #f7768e);
    border-radius: 6px;
    font-size: 12px;
    margin-top: 12px;
  }
</style>
