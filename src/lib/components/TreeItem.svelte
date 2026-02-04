<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { AgentInfo } from '$lib/stores/sessions';

  export let agent: AgentInfo;
  export let childrenMap: Map<string | null, AgentInfo[]>;
  export let depth: number = 0;
  export let selectedId: string | null = null;

  let expanded = true;
  const dispatch = createEventDispatcher<{ select: string }>();

  function getRoleName(role: AgentInfo['role']): string {
    if (role === 'Queen') return 'Queen';
    if (typeof role === 'object') {
      if ('Planner' in role) return `Planner ${role.Planner.index}`;
      if ('Worker' in role) return `Worker ${role.Worker.index}`;
      if ('Fusion' in role) return role.Fusion.variant;
    }
    return 'Agent';
  }

  function getRoleIcon(role: AgentInfo['role']): string {
    if (role === 'Queen') return '♕';
    if (typeof role === 'object') {
      if ('Planner' in role) return '◆';
      if ('Worker' in role) return '●';
      if ('Fusion' in role) return '◎';
    }
    return '○';
  }

  function getStatusIcon(status: AgentInfo['status']): string {
    if (status === 'Running') return '█';
    if (status === 'WaitingForInput') return '⏳';
    if (status === 'Completed') return '✓';
    if (status === 'Starting') return '○';
    if (typeof status === 'object' && 'Error' in status) return '✗';
    return '?';
  }

  function getStatusColor(status: AgentInfo['status']): string {
    if (status === 'Running') return 'var(--color-running)';
    if (status === 'WaitingForInput') return 'var(--color-warning)';
    if (status === 'Completed') return 'var(--color-success)';
    if (status === 'Starting') return 'var(--color-text-muted)';
    if (typeof status === 'object' && 'Error' in status) return 'var(--color-error)';
    return 'var(--color-text)';
  }

  function handleClick() {
    dispatch('select', agent.id);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      handleClick();
    }
  }

  function toggleExpand(e: Event) {
    e.stopPropagation();
    expanded = !expanded;
  }

  $: children = childrenMap.get(agent.id) || [];
  $: hasChildren = children.length > 0;
  $: isSelected = selectedId === agent.id;
  $: displayLabel = agent.config?.label || getRoleName(agent.role);
</script>

<div class="tree-item">
  <div
    class="tree-row"
    class:selected={isSelected}
    role="treeitem"
    tabindex="0"
    aria-selected={isSelected}
    aria-expanded={hasChildren ? expanded : undefined}
    on:click={handleClick}
    on:keydown={handleKeydown}
  >
    <span class="indent" style="width: {depth * 16}px"></span>

    {#if hasChildren}
      <button class="chevron" on:click={toggleExpand} aria-label={expanded ? 'Collapse' : 'Expand'}>
        {expanded ? '▼' : '▶'}
      </button>
    {:else}
      <span class="chevron-spacer"></span>
    {/if}

    <span class="role-icon">{getRoleIcon(agent.role)}</span>
    <span class="label">{displayLabel}</span>

    <span class="cli-badge">{agent.config?.cli || 'unknown'}</span>

    <span class="status-indicator" style="color: {getStatusColor(agent.status)}">
      {getStatusIcon(agent.status)}
    </span>
  </div>

  {#if hasChildren && expanded}
    <div class="children" role="group">
      {#each children as child (child.id)}
        <svelte:self
          agent={child}
          {childrenMap}
          depth={depth + 1}
          {selectedId}
          on:select
        />
      {/each}
    </div>
  {/if}
</div>

<style>
  .tree-item {
    display: flex;
    flex-direction: column;
  }

  .tree-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    cursor: pointer;
    border-radius: 4px;
    transition: background 0.15s ease;
  }

  .tree-row:hover {
    background: var(--color-bg);
  }

  .tree-row.selected {
    background: var(--color-primary-muted, rgba(139, 92, 246, 0.15));
  }

  .tree-row:focus {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--color-primary, #8b5cf6);
  }

  .indent {
    flex-shrink: 0;
  }

  .chevron {
    width: 16px;
    height: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    color: var(--color-text-muted);
    cursor: pointer;
    font-size: 8px;
    padding: 0;
    flex-shrink: 0;
    border-radius: 2px;
  }

  .chevron:hover {
    background: var(--color-border);
    color: var(--color-text);
  }

  .chevron-spacer {
    width: 16px;
    flex-shrink: 0;
  }

  .role-icon {
    font-size: 12px;
    opacity: 0.7;
    flex-shrink: 0;
  }

  .label {
    flex: 1;
    font-size: 13px;
    color: var(--color-text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .cli-badge {
    font-size: 10px;
    padding: 2px 6px;
    background: var(--color-border);
    border-radius: 3px;
    color: var(--color-text-muted);
    text-transform: lowercase;
    flex-shrink: 0;
  }

  .status-indicator {
    font-size: 10px;
    flex-shrink: 0;
  }

  .children {
    display: flex;
    flex-direction: column;
  }
</style>
