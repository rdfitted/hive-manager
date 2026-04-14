<script lang="ts">
  import { CaretDown, CaretRight, Check, ClipboardText, Crown, MagnifyingGlass, Microscope, Scales, X } from 'phosphor-svelte';
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

  function getRoleIcon(role: AgentInfo['role']) {
    if (typeof role === 'object' && role !== null) {
      if ('Judge' in role) return Scales;
      if ('Planner' in role) return '◆';
      if ('Worker' in role) return '●';
      if ('QaWorker' in role) return Microscope;
      if ('Fusion' in role) return '◎';
    }
    const k = serdeEnumVariantName(role);
    if (k === 'Queen') return Crown;
    if (k === 'Evaluator') return MagnifyingGlass;
    if (k === 'MasterPlanner') return ClipboardText;
    return '○';
  }

  function getRoleColor(role: AgentInfo['role']): string {
    if (typeof role === 'object' && role !== null) {
      if ('Worker' in role) return 'var(--accent-cyan)';
      if ('QaWorker' in role) return 'var(--accent-chrome)';
    }
    const k = serdeEnumVariantName(role);
    if (k === 'Queen') return 'var(--accent-amber)';
    if (k === 'Evaluator') return 'var(--accent-chrome)';
    return 'var(--text-secondary)';
  }

  function getStatusIcon(status: AgentInfo['status']) {
    if (typeof status === 'object' && status !== null && 'WaitingForInput' in status) return '⏳';
    if (typeof status === 'object' && status !== null && 'Error' in status) return X;
    const k = serdeEnumVariantName(status);
    if (k === 'Running') return '█';
    if (k === 'Completed') return Check;
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
  $: displayLabel = agent.config?.label
    || (agent.config?.name && agent.config?.description ? `${agent.config.name} — ${agent.config.description}` : null)
    || agent.config?.name
    || getRoleName(agent.role);
  $: RoleIcon = getRoleIcon(agent.role);
  $: StatusIcon = getStatusIcon(agent.status);
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
        {#if expanded}
          <CaretDown size={12} weight="light" />
        {:else}
          <CaretRight size={12} weight="light" />
        {/if}
      </button>
    {:else}
      <span class="chevron-spacer"></span>
    {/if}

    <span class="role-icon" style="color: {getRoleColor(agent.role)}; opacity: 1;">
      {#if typeof RoleIcon === 'string'}
        {RoleIcon}
      {:else}
        <RoleIcon size={12} weight="light" />
      {/if}
    </span>
    <span class="label">{displayLabel}</span>

    <span class="cli-badge">{agent.config?.cli || 'unknown'}</span>

    <span
      class="status-indicator"
      class:pulse-error={serdeEnumVariantName(agent.status) === 'Error'}
      style="color: {getStatusColor(agent.status, agent.role)}"
    >
      {#if typeof StatusIcon === 'string'}
        {StatusIcon}
      {:else}
        <StatusIcon size={12} weight="fill" />
      {/if}
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
    background: color-mix(in srgb, var(--accent-cyan) 15%, transparent);
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
    border-radius: var(--radius-sm);
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
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 12px;
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
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    text-transform: lowercase;
    flex-shrink: 0;
  }

  .status-indicator {
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 12px;
    font-size: 10px;
    flex-shrink: 0;
    text-shadow: 0 0 4px currentColor;
  }

  .status-indicator.pulse-error {
    animation: pulse-error 1.5s infinite;
  }

  @media (prefers-reduced-motion: reduce) {
    .status-indicator.pulse-error {
      animation: none;
    }
  }

  .children {
    display: flex;
    flex-direction: column;
  }
</style>
