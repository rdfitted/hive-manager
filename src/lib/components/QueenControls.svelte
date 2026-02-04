<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { coordination } from '$lib/stores/coordination';
  import { activeSession, activeAgents } from '$lib/stores/sessions';

  const dispatch = createEventDispatcher<{
    openAddWorker: void;
  }>();

  let selectedWorkerId: string | null = null;
  let messageInput = '';
  let sending = false;
  let error: string | null = null;

  // Get Queen ID from active session
  $: queenId = $activeSession ? `${$activeSession.id}-queen` : null;

  // Get workers (non-Queen agents)
  $: workers = $activeAgents.filter((a) => {
    if (a.role === 'Queen') return false;
    return true;
  });

  // Auto-select first worker if none selected
  $: if (!selectedWorkerId && workers.length > 0) {
    selectedWorkerId = workers[0].id;
  }

  async function handleSend() {
    if (!$activeSession?.id || !queenId || !selectedWorkerId || !messageInput.trim()) {
      return;
    }

    sending = true;
    error = null;

    try {
      await coordination.queenInject(
        $activeSession.id,
        queenId,
        selectedWorkerId,
        messageInput.trim()
      );
      messageInput = '';
    } catch (err) {
      error = String(err);
    } finally {
      sending = false;
    }
  }

  async function handleBroadcast() {
    if (!$activeSession?.id || !queenId || !messageInput.trim()) {
      return;
    }

    sending = true;
    error = null;

    try {
      for (const worker of workers) {
        await coordination.queenInject(
          $activeSession.id,
          queenId,
          worker.id,
          messageInput.trim()
        );
      }
      messageInput = '';
    } catch (err) {
      error = String(err);
    } finally {
      sending = false;
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      handleSend();
    }
  }

  function getWorkerLabel(worker: { id: string; config: { label?: string }; role: unknown }): string {
    if (worker.config.label) return worker.config.label;

    // Extract role info
    if (worker.role && typeof worker.role === 'object' && 'Worker' in worker.role) {
      const idx = (worker.role as { Worker: { index: number } }).Worker.index;
      return `Worker ${idx}`;
    }
    if (worker.role && typeof worker.role === 'object' && 'Planner' in worker.role) {
      const idx = (worker.role as { Planner: { index: number } }).Planner.index;
      return `Planner ${idx}`;
    }

    return worker.id.split('-').pop() || worker.id;
  }
</script>

<div class="queen-controls">
  <div class="controls-header">
    <h4>Queen Controls</h4>
    <button class="add-worker-btn" on:click={() => dispatch('openAddWorker')} title="Add Worker">
      + Add Worker
    </button>
  </div>

  {#if !$activeSession}
    <div class="no-session">No active session</div>
  {:else if workers.length === 0}
    <div class="no-workers">No workers available</div>
  {:else}
    <div class="target-section">
      <label for="target-select">Target:</label>
      <select id="target-select" bind:value={selectedWorkerId} class="target-select">
        {#each workers as worker (worker.id)}
          <option value={worker.id}>
            {getWorkerLabel(worker)}
          </option>
        {/each}
      </select>
    </div>

    <div class="message-section">
      <textarea
        placeholder="Enter message for worker..."
        bind:value={messageInput}
        on:keydown={handleKeydown}
        rows={2}
        class="message-input"
        disabled={sending}
      ></textarea>
    </div>

    {#if error}
      <div class="error">{error}</div>
    {/if}

    <div class="action-buttons">
      <button
        class="send-btn"
        on:click={handleSend}
        disabled={sending || !messageInput.trim()}
      >
        {sending ? 'Sending...' : 'Send'}
      </button>
      <button
        class="broadcast-btn"
        on:click={handleBroadcast}
        disabled={sending || !messageInput.trim()}
        title="Send to all workers"
      >
        Broadcast
      </button>
    </div>

    <div class="quick-actions">
      <span class="quick-label">Quick:</span>
      <button
        class="quick-btn"
        on:click={() => {
          messageInput = 'What is your current status?';
        }}
      >
        Status
      </button>
      <button
        class="quick-btn"
        on:click={() => {
          messageInput = 'Please pause your current work.';
        }}
      >
        Pause
      </button>
      <button
        class="quick-btn"
        on:click={() => {
          messageInput = 'Continue with your task.';
        }}
      >
        Continue
      </button>
    </div>
  {/if}
</div>

<style>
  .queen-controls {
    padding: 12px;
    background: var(--bg-secondary, #1a1b26);
    border-radius: 8px;
  }

  .controls-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 12px;
  }

  .controls-header h4 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary, #c0caf5);
  }

  .add-worker-btn {
    padding: 4px 10px;
    font-size: 11px;
    background: var(--accent-color, #7aa2f7);
    border: none;
    border-radius: 4px;
    color: white;
    cursor: pointer;
    font-weight: 500;
  }

  .add-worker-btn:hover {
    opacity: 0.9;
  }

  .no-session,
  .no-workers {
    color: var(--text-secondary, #565f89);
    font-size: 12px;
    text-align: center;
    padding: 16px;
    font-style: italic;
  }

  .target-section {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 10px;
  }

  .target-section label {
    font-size: 12px;
    color: var(--text-secondary, #565f89);
    flex-shrink: 0;
  }

  .target-select {
    flex: 1;
    padding: 6px 10px;
    background: var(--bg-tertiary, #24283b);
    border: 1px solid var(--border-color, #414868);
    border-radius: 4px;
    color: var(--text-primary, #c0caf5);
    font-size: 12px;
  }

  .target-select:focus {
    outline: none;
    border-color: var(--accent-color, #7aa2f7);
  }

  .message-section {
    margin-bottom: 10px;
  }

  .message-input {
    width: 100%;
    padding: 8px 10px;
    background: var(--bg-tertiary, #24283b);
    border: 1px solid var(--border-color, #414868);
    border-radius: 4px;
    color: var(--text-primary, #c0caf5);
    font-size: 12px;
    font-family: inherit;
    resize: vertical;
    min-height: 40px;
  }

  .message-input:focus {
    outline: none;
    border-color: var(--accent-color, #7aa2f7);
  }

  .message-input:disabled {
    opacity: 0.6;
  }

  .error {
    padding: 6px 10px;
    background: var(--error-bg, #3b2030);
    color: var(--error-text, #f7768e);
    border-radius: 4px;
    font-size: 11px;
    margin-bottom: 10px;
  }

  .action-buttons {
    display: flex;
    gap: 8px;
    margin-bottom: 12px;
  }

  .send-btn,
  .broadcast-btn {
    flex: 1;
    padding: 8px 12px;
    border: none;
    border-radius: 4px;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .send-btn {
    background: var(--accent-color, #7aa2f7);
    color: white;
  }

  .broadcast-btn {
    background: var(--bg-tertiary, #24283b);
    color: var(--text-primary, #c0caf5);
    border: 1px solid var(--border-color, #414868);
  }

  .send-btn:hover:not(:disabled),
  .broadcast-btn:hover:not(:disabled) {
    opacity: 0.9;
  }

  .send-btn:disabled,
  .broadcast-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .quick-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .quick-label {
    font-size: 11px;
    color: var(--text-secondary, #565f89);
  }

  .quick-btn {
    padding: 4px 8px;
    font-size: 10px;
    background: var(--bg-tertiary, #24283b);
    border: 1px solid var(--border-color, #414868);
    border-radius: 4px;
    color: var(--text-secondary, #a9b1d6);
    cursor: pointer;
  }

  .quick-btn:hover {
    background: var(--bg-hover, #292e42);
    color: var(--text-primary, #c0caf5);
  }
</style>
