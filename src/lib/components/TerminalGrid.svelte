<script lang="ts">
  import type { AgentInfo } from '$lib/stores/sessions';
  import Terminal from './Terminal.svelte';

  interface Props {
    agents: AgentInfo[];
    focusedAgentId: string | null;
    onSelect: (id: string) => void;
  }

  let { agents, focusedAgentId, onSelect }: Props = $props();

  let cols = $derived(
    agents.length <= 1 ? 1 :
    agents.length <= 2 ? 2 :
    agents.length <= 4 ? 2 :
    agents.length <= 6 ? 3 :
    agents.length <= 9 ? 3 :
    4
  );

  let rows = $derived(
    agents.length <= 2 ? 1 :
    agents.length <= 4 ? 2 :
    agents.length <= 6 ? 2 :
    agents.length <= 9 ? 3 :
    Math.ceil(agents.length / 4)
  );

  function getRoleLabel(agent: AgentInfo) {
    if (agent.config?.label) return agent.config.label;
    if (agent.role === 'Queen') return 'Queen';
    if (typeof agent.role === 'object') {
      if ('Planner' in agent.role) return `Planner ${agent.role.Planner.index}`;
      if ('Worker' in agent.role) return `Worker ${agent.role.Worker.index}`;
    }
    return 'Agent';
  }
</script>

<div 
  class="terminal-grid" 
  style="--cols: {cols}; --rows: {rows}"
  class:scrollable={agents.length > 9}
>
  {#each agents as agent (agent.id)}
    <div 
      class="terminal-item" 
      class:focused={agent.id === focusedAgentId}
      onclick={() => onSelect(agent.id)}
    >
      <div class="terminal-header">
        <span class="role-label">{getRoleLabel(agent)}</span>
        <div class="terminal-meta">
          <span class="cli-badge">{agent.config?.cli || 'unknown'}</span>
          <span class="status-indicator" 
            class:waiting={typeof agent.status === 'object' && 'WaitingForInput' in agent.status} 
            class:running={agent.status === 'Running'} 
            class:completed={agent.status === 'Completed'}
          >
             {agent.status === 'Running' ? '█' : 
              (typeof agent.status === 'object' && 'WaitingForInput' in agent.status) ? '⏳' : 
              agent.status === 'Completed' ? '✓' : '○'}
          </span>
        </div>
      </div>
      <div class="terminal-container">
        <Terminal agentId={agent.id} isFocused={agent.id === focusedAgentId} />
      </div>
    </div>
  {/each}
</div>

<style>
  .terminal-grid {
    display: grid;
    grid-template-columns: repeat(var(--cols), 1fr);
    grid-template-rows: repeat(var(--rows), 1fr);
    gap: 12px;
    width: 100%;
    height: 100%;
    padding: 4px;
    overflow: hidden;
  }

  .terminal-grid.scrollable {
    grid-template-rows: repeat(var(--rows), 300px);
    overflow-y: auto;
  }

  .terminal-item {
    display: flex;
    flex-direction: column;
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 8px;
    overflow: hidden;
    transition: border-color 0.2s, box-shadow 0.2s;
    min-height: 0;
  }

  .terminal-item.focused {
    border-color: var(--color-accent);
    box-shadow: 0 0 0 1px var(--color-accent);
  }

  .terminal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 10px;
    background: var(--color-surface);
    border-bottom: 1px solid var(--color-border);
    user-select: none;
  }

  .role-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--color-text);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .terminal-meta {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .cli-badge {
    font-size: 9px;
    padding: 1px 4px;
    background: var(--color-border);
    border-radius: 3px;
    color: var(--color-text-muted);
    text-transform: lowercase;
  }

  .status-indicator {
    font-size: 10px;
  }

  .status-indicator.running {
    color: var(--color-running);
  }

  .status-indicator.waiting {
    color: var(--color-warning);
    animation: pulse 2s infinite;
  }

  .status-indicator.completed {
    color: var(--color-success);
  }

  @keyframes pulse {
    0% { opacity: 1; }
    50% { opacity: 0.5; }
    100% { opacity: 1; }
  }

  .terminal-container {
    flex: 1;
    min-height: 0;
  }
</style>
