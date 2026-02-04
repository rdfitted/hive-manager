<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { activeSession, activeAgents, type AgentStatus } from '$lib/stores/sessions';

  const dispatch = createEventDispatcher<{
    openAddWorker: void;
  }>();

  function getAgentLabel(agent: { id: string; config: { label?: string }; role: unknown }): string {
    if (agent.config?.label) return agent.config.label;
    if (agent.role === 'Queen') return 'Queen';
    if (agent.role && typeof agent.role === 'object' && 'Worker' in agent.role) {
      const idx = (agent.role as { Worker: { index: number } }).Worker.index;
      return `Worker ${idx}`;
    }
    if (agent.role && typeof agent.role === 'object' && 'Planner' in agent.role) {
      const idx = (agent.role as { Planner: { index: number } }).Planner.index;
      return `Planner ${idx}`;
    }
    return agent.id.split('-').pop() || agent.id;
  }

  function getAgentStatusInfo(status: AgentStatus): { icon: string; color: string } {
    if (status === 'Running') return { icon: '●', color: 'var(--color-running, #7aa2f7)' };
    if (status === 'Completed') return { icon: '✓', color: 'var(--color-success, #9ece6a)' };
    if (status === 'Starting') return { icon: '◐', color: 'var(--text-secondary, #565f89)' };
    if (typeof status === 'object' && 'WaitingForInput' in status) return { icon: '⏳', color: 'var(--color-warning, #e0af68)' };
    if (typeof status === 'object' && 'Error' in status) return { icon: '✗', color: 'var(--color-error, #f7768e)' };
    return { icon: '○', color: 'var(--text-secondary, #565f89)' };
  }
</script>

<div class="queen-controls">
  <div class="controls-header">
    <h4>Session Controls</h4>
    <button class="add-worker-btn" on:click={() => dispatch('openAddWorker')} title="Add Worker">
      + Add Worker
    </button>
  </div>

  {#if !$activeSession}
    <div class="no-session">No active session</div>
  {:else if $activeAgents.length === 0}
    <div class="no-workers">No agents available</div>
  {:else}
    <div class="agents-status">
      <div class="status-header">Agent Status</div>
      {#each $activeAgents as agent (agent.id)}
        {@const statusInfo = getAgentStatusInfo(agent.status)}
        <div class="agent-row">
          <span class="agent-icon" style="color: {statusInfo.color}">{statusInfo.icon}</span>
          <span class="agent-name">{getAgentLabel(agent)}</span>
          <span class="agent-cli">{agent.config?.cli || 'claude'}</span>
        </div>
      {/each}
    </div>

    <div class="info-note">
      Agents coordinate via file-based polling. Click on an agent in the tree above to view its terminal.
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

  .agents-status {
    margin-bottom: 12px;
  }

  .status-header {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--text-secondary, #565f89);
    margin-bottom: 8px;
  }

  .agent-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    background: var(--bg-tertiary, #24283b);
    border-radius: 4px;
    margin-bottom: 4px;
  }

  .agent-row:last-child {
    margin-bottom: 0;
  }

  .agent-icon {
    font-size: 10px;
  }

  .agent-name {
    flex: 1;
    font-size: 12px;
    color: var(--text-primary, #c0caf5);
  }

  .agent-cli {
    font-size: 10px;
    padding: 2px 6px;
    background: var(--bg-secondary, #1a1b26);
    border-radius: 3px;
    color: var(--text-secondary, #565f89);
  }

  .info-note {
    font-size: 11px;
    color: var(--text-secondary, #565f89);
    padding: 8px;
    background: var(--bg-tertiary, #24283b);
    border-radius: 4px;
    line-height: 1.4;
  }
</style>
