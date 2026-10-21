<script lang="ts">
  import { CaretDown, CaretRight } from 'phosphor-svelte';
  import { createEventDispatcher } from 'svelte';
  import { serdeEnumVariantName, type AgentInfo } from '$lib/stores/sessions';
  import { heartbeatStore } from '$lib/stores/conversations';
  import { attentionReason, needsAttention } from '$lib/stores/agentActivity';
  import AgentActivityLine from './agent/AgentActivityLine.svelte';

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
    if (e.target === e.currentTarget && (e.key === 'Enter' || e.key === ' ')) {
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
  $: attention = needsAttention(agent, $heartbeatStore.staleAgents);
  $: reason = attentionReason(agent, $heartbeatStore.staleAgents);
  $: reasonLabel = reason === 'delivery_failed' ? 'Delivery failed' : reason === 'error' ? 'Agent error' : 'Stale heartbeat';
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
      <button
        class="lattice-btn lattice-btn--ghost lattice-btn--icon"
        on:click={toggleExpand}
        aria-label={expanded ? 'Collapse' : 'Expand'}
      >
        {#if expanded}
          <CaretDown size={12} weight="light" />
        {:else}
          <CaretRight size={12} weight="light" />
        {/if}
      </button>
    {:else}
      <span class="chevron-spacer"></span>
    {/if}

    <span class="status-dot" role="img"
      aria-label={serdeEnumVariantName(agent.status) || 'Unknown status'}
      title={serdeEnumVariantName(agent.status) || 'Unknown status'}
      style="color: {getStatusColor(agent.status, agent.role)}"
    ></span>
    <span class="agent-copy">
      <span class="label">{displayLabel}</span>
      <AgentActivityLine agentId={agent.id} compact />
    </span>
    {#if attention}
      <button type="button" class="attention-badge lattice-btn lattice-btn--secondary"
        aria-label={`Needs you: ${reasonLabel} — ${displayLabel}`} title={reasonLabel}
        on:click={(event) => { event.stopPropagation(); handleClick(); }}
      >Needs you</button>
    {/if}
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
    border-radius: var(--radius-md);
    transition: background-color var(--motion-duration-fast) ease;
  }

  .tree-row:hover {
    background: var(--bg-void);
  }

  .tree-row.selected {
    background: color-mix(in srgb, var(--accent-cyan) 15%, transparent);
  }

  .tree-row:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent-cyan);
  }

  .indent {
    flex-shrink: 0;
  }

  .chevron-spacer {
    width: 16px;
    flex-shrink: 0;
  }

  .agent-copy { display: flex; flex: 1; flex-direction: column; min-width: 0; }

  .label {
    font-size: 13px;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .attention-badge { font-size: 10px; padding: 2px 5px; min-height: 22px; flex-shrink: 0; }

  .status-dot { width: 7px; height: 7px; border: 1px solid currentColor; border-radius: 50%; background: currentColor; flex-shrink: 0; }

  @media (forced-colors: active) {
    .status-dot { background: CanvasText; border-color: CanvasText; }
    .attention-badge { border: 1px solid ButtonText; }
  }

  .children {
    display: flex;
    flex-direction: column;
  }
</style>
