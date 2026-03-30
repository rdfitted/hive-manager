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
      if ('Planner' in role) return `Planner ${role.Planner.index}`;
      if ('Worker' in role) return `Worker ${role.Worker.index}`;
      if ('QaWorker' in role) return `QA Worker ${role.QaWorker.index}`;
      if ('Fusion' in role) return role.Fusion.variant;
    }
    const k = serdeEnumVariantName(role);
    if (k === 'Queen') return 'Queen';
    if (k === 'Evaluator') return 'Evaluator';
    if (k === 'Judge') return 'Judge';
    if (k === 'MasterPlanner') return 'Master Planner';
    return 'Agent';
  }

  function getRoleIcon(role: AgentInfo['role']): string {
    if (typeof role === 'object' && role !== null) {
      if ('Planner' in role) return '◆';
      if ('Worker' in role) return '●';
      if ('QaWorker' in role) return '🔬';
      if ('Fusion' in role) return '◎';
    }
    const k = serdeEnumVariantName(role);
    if (k === 'Queen') return '♕';
    if (k === 'Evaluator') return '🔍';
    if (k === 'Judge') return '⚖';
    if (k === 'MasterPlanner') return '📋';
    return '○';
  }

  function getRoleColor(role: AgentInfo['role']): string {
    if (typeof role === 'object' && role !== null && 'QaWorker' in role) return '#9333ea'; // Purple
    const k = serdeEnumVariantName(role);
    if (k === 'Queen') return 'var(--color-primary)';
    if (k === 'Evaluator') return '#d946ef'; // Fuchsia
    return 'var(--color-text-muted)';
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
      if (sk === 'Running') return rk === 'Evaluator' ? '#d946ef' : '#9333ea';
    }

    if (sk === 'Running') return 'var(--color-running)';
    if (typeof status === 'object' && status !== null && 'WaitingForInput' in status)
      return 'var(--color-warning)';
    if (sk === 'Completed') return 'var(--color-success)';
    if (sk === 'Starting') return 'var(--color-text-muted)';
    if (typeof status === 'object' && status !== null && 'Error' in status) return 'var(--color-error)';
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
