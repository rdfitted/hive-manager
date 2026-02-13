<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { heartbeatStore } from '$lib/stores/conversations';
  import { activeSession } from '$lib/stores/sessions';

  let interval: ReturnType<typeof setInterval>;

  $: sessionId = $activeSession?.id;
  $: agents = $activeSession?.agents ?? [];

  // Poll heartbeats
  $: if (sessionId) {
    heartbeatStore.loadHeartbeats(sessionId);
  }

  onMount(() => {
    interval = setInterval(() => {
      if (sessionId) heartbeatStore.loadHeartbeats(sessionId);
    }, 10000);
  });

  onDestroy(() => {
    clearInterval(interval);
  });

  function getStatusColor(agentId: string, agentStatus: string): string {
    if ($heartbeatStore.stalledAgents.has(agentId)) return '#f7768e'; // red
    if (agentStatus === 'Running') return '#9ece6a'; // green
    if (agentStatus === 'Starting') return '#e0af68'; // yellow
    if (agentStatus === 'Completed') return '#565f89'; // dim
    return '#e0af68'; // yellow default
  }

  function getStatusLabel(agentId: string, agentStatus: string): string {
    if ($heartbeatStore.stalledAgents.has(agentId)) return 'STALLED';
    return agentStatus;
  }

  function getRoleName(role: unknown): string {
    if (role === 'Queen') return 'Queen';
    if (role === 'MasterPlanner') return 'Planner';
    if (typeof role === 'object' && role !== null) {
      if ('Worker' in (role as Record<string, unknown>)) return 'Worker';
      if ('Planner' in (role as Record<string, unknown>)) return 'Planner';
      if ('Fusion' in (role as Record<string, unknown>)) return 'Fusion';
    }
    return 'Agent';
  }

  function formatTime(ts: string): string {
    if (!ts) return '';
    const d = new Date(ts);
    return d.toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }

  function getHeartbeat(agentId: string) {
    return $heartbeatStore.agents[agentId];
  }
</script>

{#if agents.length > 0}
  <div class="agent-status-bar">
    {#each agents as agent (agent.id)}
      {@const statusStr = typeof agent.status === 'string' ? agent.status : 'Unknown'}
      {@const hb = getHeartbeat(agent.id)}
      <div class="agent-chip" title={hb?.summary || statusStr}>
        <span
          class="status-dot"
          style="background-color: {getStatusColor(agent.id, statusStr)}"
        ></span>
        <span class="agent-name">
          {agent.config?.label || getRoleName(agent.role)}
        </span>
        <span class="agent-status">{getStatusLabel(agent.id, statusStr)}</span>
        {#if hb}
          <span class="agent-time">{formatTime(hb.timestamp)}</span>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .agent-status-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 8px 12px;
    background: var(--bg-tertiary, #24283b);
    border-bottom: 1px solid var(--border-color, #414868);
  }

  .agent-chip {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    background: var(--bg-primary, #1a1b26);
    border: 1px solid var(--border-color, #414868);
    border-radius: 12px;
    font-size: 11px;
    cursor: default;
  }

  .status-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .agent-name {
    color: var(--text-primary, #c0caf5);
    font-weight: 600;
  }

  .agent-status {
    color: var(--text-secondary, #a9b1d6);
    font-size: 10px;
    text-transform: uppercase;
  }

  .agent-time {
    color: var(--text-muted, #3b4261);
    font-size: 10px;
  }
</style>
