<script lang="ts">
  import { activeSession, sessions } from '$lib/stores/sessions';
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

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
    rawContent: string;
  }

  let plan: Plan | null = $state(null);
  let loading = $state(false);
  let continuing = $state(false);
  let sendingRefinement = $state(false);
  let refinementInput = $state('');
  let error = $state<string | null>(null);
  let lastSessionId: string | null = null;
  let pollInterval: ReturnType<typeof setInterval> | null = null;

  // Check if session is in a planning-related state
  function isPlanning(): boolean {
    return $activeSession?.state === 'Planning';
  }

  function isPlanReady(): boolean {
    return $activeSession?.state === 'PlanReady';
  }

  // Check if we're in an interactive planning state (Planning or PlanReady with Master Planner still running)
  function canRefine(): boolean {
    if (!$activeSession) return false;
    const state = $activeSession.state;
    if (state !== 'Planning' && state !== 'PlanReady') return false;
    // Check if Master Planner agent exists and is running
    const masterPlanner = $activeSession.agents.find(a => a.role === 'MasterPlanner');
    return masterPlanner?.status === 'Running';
  }

  async function handleContinue() {
    if (!$activeSession) return;
    continuing = true;
    error = null;
    try {
      await sessions.continueAfterPlanning($activeSession.id);
    } catch (e) {
      error = String(e);
    } finally {
      continuing = false;
    }
  }

  async function handleRefinement() {
    if (!$activeSession || !refinementInput.trim()) return;

    sendingRefinement = true;
    error = null;

    try {
      // Find the Master Planner agent
      const masterPlanner = $activeSession.agents.find(a => a.role === 'MasterPlanner');
      if (!masterPlanner) {
        throw new Error('Master Planner not found');
      }

      // Send refinement request to Master Planner's PTY
      const message = `\n\n---\n**User Feedback**: ${refinementInput.trim()}\n\nPlease refine the plan based on this feedback and update plan.md.\n---\n\n`;
      await invoke('write_to_pty', { id: masterPlanner.id, data: message });

      refinementInput = '';
    } catch (e) {
      error = String(e);
    } finally {
      sendingRefinement = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleRefinement();
    }
  }

  // Start polling for plan
  function startPolling() {
    const state = $activeSession?.state;
    const interval = state === 'Running' ? 5000 : 2000;
    
    // If interval already exists, check if it's the right frequency
    // For simplicity, we'll just restart it if the state changed significantly
    if (pollInterval) {
      // We don't want to restart on every effect pulse, 
      // so we only restart if we're switching modes
      // But since stopPolling is called in the effect when switching away from Planning/Ready/Running,
      // we only need to worry about transitions between these three.
      return; 
    }
    
    pollInterval = setInterval(() => {
      if ($activeSession?.id) {
        loadPlan($activeSession.id);
      }
    }, interval);
  }

  function stopPolling() {
    if (pollInterval) {
      clearInterval(pollInterval);
      pollInterval = null;
    }
  }

  onMount(() => {
    const unlisten = listen('plan-update', (event) => {
      console.log('Plan update event received:', event);
      if ($activeSession?.id) {
        loadPlan($activeSession.id);
      }
    });

    return () => {
      unlisten.then(fn => fn());
    };
  });

  onDestroy(() => {
    stopPolling();
  });

  // Load plan when session changes and manage polling
  $effect(() => {
    const sessionId = $activeSession?.id;
    const state = $activeSession?.state;

    if (sessionId && sessionId !== lastSessionId) {
      lastSessionId = sessionId;
      loadPlan(sessionId);
    } else if (!sessionId) {
      plan = null;
      lastSessionId = null;
      stopPolling();
    }

    // Start/stop polling based on state
    if (state === 'Planning' || state === 'PlanReady' || state === 'Running') {
      startPolling();
    } else {
      stopPolling();
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
  {:else if isPlanning() && !plan}
    <div class="planning-state">
      <div class="planning-header">
        <span class="planning-icon">🧠</span>
        <h3>Master Planner Working</h3>
      </div>
      <p class="planning-description">
        The Master Planner is analyzing your project and creating a detailed implementation plan...
      </p>
      <div class="planning-progress">
        <span class="spinner large">◐</span>
        <span>Generating plan.md</span>
      </div>
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
      {#if plan.summary}
        <p class="summary">{plan.summary}</p>
      {/if}
      <span class="timestamp">Last updated: {new Date(plan.generatedAt).toLocaleString()}</span>
    </div>

    {#if plan.tasks.length > 0}
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
    {:else}
      <!-- Show raw markdown when no tasks parsed yet (plan in progress) -->
      <div class="raw-content">
        <div class="raw-header">
          <span class="raw-icon">📄</span>
          <span class="raw-label">Plan Content</span>
          {#if isPlanning()}
            <span class="writing-indicator">
              <span class="spinner">◐</span>
              Writing...
            </span>
          {/if}
        </div>
        <pre class="raw-markdown">{plan.rawContent}</pre>
      </div>
    {/if}

    {#if isPlanning() || isPlanReady()}
      <div class="plan-actions">
        {#if canRefine()}
          <div class="refinement-section">
            <p class="refinement-hint">
              Not quite right? Ask the Master Planner to refine the plan:
            </p>
            <div class="refinement-input-group">
              <input
                type="text"
                class="refinement-input"
                placeholder="e.g., Focus more on the backend API..."
                bind:value={refinementInput}
                onkeydown={handleKeydown}
                disabled={sendingRefinement}
              />
              <button
                class="refinement-button"
                onclick={handleRefinement}
                disabled={sendingRefinement || !refinementInput.trim()}
              >
                {#if sendingRefinement}
                  <span class="spinner">◐</span>
                {:else}
                  Refine
                {/if}
              </button>
            </div>
          </div>
        {/if}

        <div class="approve-section">
          <p class="plan-ready-hint">
            {#if isPlanning()}
              Happy with the plan? Approve it to spawn the Queen and Workers.
            {:else}
              Review the plan above. When ready, click Continue to spawn the Queen and Workers.
            {/if}
          </p>
          <button
            class="continue-button"
            onclick={handleContinue}
            disabled={continuing}
          >
            {#if continuing}
              <span class="spinner">◐</span>
              Launching...
            {:else}
              Approve & Continue
            {/if}
          </button>
        </div>
      </div>
    {/if}
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

  /* Raw content display (for plans in progress) */
  .raw-content {
    margin-top: 12px;
  }

  .raw-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--border-color, #414868);
  }

  .raw-icon {
    font-size: 16px;
  }

  .raw-label {
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--text-secondary, #565f89);
  }

  .writing-indicator {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
    font-size: 11px;
    color: var(--accent-color, #7aa2f7);
  }

  .raw-markdown {
    margin: 0;
    padding: 16px;
    background: var(--bg-tertiary, #24283b);
    border-radius: 6px;
    font-size: 12px;
    font-family: 'Fira Code', 'Monaco', monospace;
    color: var(--text-primary, #c0caf5);
    white-space: pre-wrap;
    word-wrap: break-word;
    max-height: 400px;
    overflow-y: auto;
    line-height: 1.5;
  }

  /* Planning state styles */
  .planning-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 48px 24px;
    text-align: center;
  }

  .planning-header {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 16px;
  }

  .planning-icon {
    font-size: 32px;
  }

  .planning-header h3 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--text-primary, #c0caf5);
  }

  .planning-description {
    margin: 0 0 24px 0;
    color: var(--text-secondary, #a9b1d6);
    font-size: 14px;
    max-width: 300px;
    line-height: 1.5;
  }

  .planning-progress {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 20px;
    background: var(--bg-tertiary, #24283b);
    border-radius: 8px;
    color: var(--accent-color, #7aa2f7);
    font-size: 13px;
  }

  .spinner.large {
    font-size: 18px;
  }

  /* Plan actions */
  .plan-actions {
    margin-top: 24px;
    padding-top: 20px;
    border-top: 1px solid var(--border-color, #414868);
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .refinement-section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .refinement-hint {
    margin: 0;
    color: var(--text-secondary, #a9b1d6);
    font-size: 12px;
  }

  .refinement-input-group {
    display: flex;
    gap: 8px;
  }

  .refinement-input {
    flex: 1;
    padding: 10px 12px;
    background: var(--bg-tertiary, #24283b);
    border: 1px solid var(--border-color, #414868);
    border-radius: 6px;
    color: var(--text-primary, #c0caf5);
    font-size: 13px;
  }

  .refinement-input:focus {
    outline: none;
    border-color: var(--accent-color, #7aa2f7);
  }

  .refinement-input::placeholder {
    color: var(--text-muted, #565f89);
  }

  .refinement-input:disabled {
    opacity: 0.6;
  }

  .refinement-button {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 10px 16px;
    background: var(--bg-tertiary, #24283b);
    border: 1px solid var(--border-color, #414868);
    border-radius: 6px;
    color: var(--text-primary, #c0caf5);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
    white-space: nowrap;
  }

  .refinement-button:hover:not(:disabled) {
    border-color: var(--accent-color, #7aa2f7);
    background: var(--bg-secondary, #1f2335);
  }

  .refinement-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .approve-section {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding-top: 16px;
    border-top: 1px dashed var(--border-color, #414868);
  }

  .plan-ready-hint {
    margin: 0;
    text-align: center;
    color: var(--text-secondary, #a9b1d6);
    font-size: 13px;
    max-width: 280px;
    line-height: 1.5;
  }

  .continue-button {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 12px 28px;
    background: var(--color-success, #9ece6a);
    color: var(--bg-primary, #1a1b26);
    border: none;
    border-radius: 8px;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.15s, opacity 0.15s;
  }

  .continue-button:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .continue-button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
