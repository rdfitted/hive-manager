<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { serdeEnumVariantName, type AgentInfo } from '$lib/stores/sessions';

  export let agent: AgentInfo;
  export let childrenMap: Map<string | null, AgentInfo[]>;
  export let depth: number = 0;
  export let selectedId: string | null = null;

  let expanded = true;
  const dispatch = createEventDispatcher<{ select: string }>();

  function getRoleName(role: AgentInfo['role']): string {
    if (typeof role === 'object' && role !== null) {
      if ('Judge' in role) return 'Judge';
      if ('Planner' in role) return `Planner ${role.Planner.index}`;
      if ('Worker' in role) return `Worker ${role.Worker.index}`;
      if ('QaWorker' in role) return `QA Worker ${role.QaWorker.index}`;
      if ('Fusion' in role) return role.Fusion.variant;
    }
    const k = serdeEnumVariantName(role);
    if (k === 'Queen') return 'Queen';
    if (k === 'Evaluator') return 'Evaluator';
    if (k === 'MasterPlanner') return 'Master Planner';
    return 'Agent';
  }

  function getRoleIcon(role: AgentInfo['role']): string {
    if (typeof role === 'object' && role !== null) {
      if ('Judge' in role) return '⚖';
      if ('Planner' in role) return '◆';
      if ('Worker' in role) return '●';
      if ('QaWorker' in role) return '🔬';
      if ('Fusion' in role) return '◎';
    }
    const k = serdeEnumVariantName(role);
    if (k === 'Queen') return '♕';
    if (k === 'Evaluator') return '🔍';
    if (k === 'MasterPlanner') return '📋';
    return '○';
  }

  function getRoleColor(role: AgentInfo['role']): string {
    if (typeof role === 'object' && role !== null && 'QaWorker' in role) return 'var(--accent-cyan)';
    const k = serdeEnumVariantName(role);
    if (k === 'Queen') return 'var(--accent-cyan)';
    if (k === 'Evaluator') return 'var(--accent-cyan)';
    return 'var(--text-secondary)';
  }

  function getStatusIcon(status: AgentInfo['status']): string {
    if (typeof status === 'object' && status !== null && 'WaitingForInput' in status) return '⏳';
    if (typeof status === 'object' && status !== null && 'Error' in status) return '✗';
    const k = serdeEnumVariantName(status);
    if (k === 'Running') return '█';
    if (k === 'Completed') return '✓';
    if (k === 'Starting') return '○';
    return '?';
  }

  function getStatusColor(status: AgentInfo['status'], role: AgentInfo['role']): string {
    const rk = serdeEnumVariantName(role);
    const sk = serdeEnumVariantName(status);
    const isQaWorkerRole = typeof role === 'object' && role !== null && 'QaWorker' in role;
    if (rk === 'Evaluator' || isQaWorkerRole) {
      if (sk === 'Running') return 'var(--accent-cyan)';
    }

    if (sk === 'Running') return 'var(--accent-cyan)';
    if (typeof status === 'object' && status !== null && 'WaitingForInput' in status)
      return 'var(--status-warning)';
    if (sk === 'Completed') return 'var(--status-success)';
    if (sk === 'Starting') return 'var(--text-secondary)';
    if (typeof status === 'object' && status !== null && 'Error' in status) return 'var(--status-error)';
    return 'var(--text-primary)';
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

    <span class="role-icon" style="color: {getRoleColor(agent.role)}; opacity: 1;">{getRoleIcon(agent.role)}</span>
    <span class="label">{displayLabel}</span>

    <span class="cli-badge">{agent.config?.cli || 'unknown'}</span>

    <span class="status-indicator" style="color: {getStatusColor(agent.status, agent.role)}">
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
    border-radius: var(--radius-sm);
    transition: background 0.15s ease;
  }

  .tree-row:hover {
    background: var(--bg-void);
  }

  .tree-row.selected {
    background: rgba(125, 207, 255, 0.15);
  }

  .tree-row:focus {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent-cyan);
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
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 8px;
    padding: 0;
    flex-shrink: 0;
    border-radius: 2px;
  }

  .chevron:hover {
    background: var(--border-structural);
    color: var(--text-primary);
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
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .cli-badge {
    font-size: 10px;
    padding: 2px 6px;
    background: var(--border-structural);
    border-radius: 3px;
    color: var(--text-secondary);
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
