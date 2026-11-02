<script lang="ts">
  import { CaretDown, CaretRight } from 'phosphor-svelte';
  import { createEventDispatcher } from 'svelte';
  import { serdeEnumVariantName, type AgentInfo } from '$lib/stores/sessions';
  import { heartbeatStore } from '$lib/stores/conversations';
  import { attentionReason, needsAttention, workStateFor } from '$lib/stores/agentActivity';
  import AgentActivityLine from './agent/AgentActivityLine.svelte';
  import type { AgentDependencies } from '$lib/stores/agentDependencies';

  export let agent: AgentInfo;
  export let childrenMap: Map<string | null, AgentInfo[]>;
  export let sessionAgentIds: ReadonlySet<string>;
  export let depth: number = 0;
  export let selectedId: string | null = null;
  export let byAgent: Map<string, AgentDependencies> = new Map();

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
      if (sk === 'Running') return 'var(--accent)';
    }

    if (sk === 'Running') return 'var(--accent)';
    if (typeof status === 'object' && status !== null && 'WaitingForInput' in status)
      return 'var(--status-warning)';
    if (sk === 'Completed') return 'var(--status-success)';
    if (sk === 'Starting') return 'var(--text-muted)';
    if (typeof status === 'object' && status !== null && 'Error' in status) return 'var(--status-error)';
    return 'var(--text-primary)';
  }

  function handleClick() {
    dispatch('select', agent.id);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.target !== e.currentTarget || e.isComposing || e.altKey || e.ctrlKey || e.metaKey) return;
    const row = e.currentTarget as HTMLElement;
    if (e.key === 'ArrowRight') {
      e.preventDefault();
      if (hasChildren && !expanded) expanded = true;
      else row.parentElement?.querySelector<HTMLElement>('[role="group"] [role="treeitem"]')?.focus();
    } else if (e.key === 'ArrowLeft') {
      e.preventDefault();
      if (hasChildren && expanded) expanded = false;
      else row.parentElement?.parentElement?.closest('.tree-item')?.querySelector<HTMLElement>('[role="treeitem"]')?.focus();
    }
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
  $: attention = needsAttention(agent, $heartbeatStore.staleAgents, workStateFor(agent, $heartbeatStore.agents[agent.id]));
  $: reason = attentionReason(agent, $heartbeatStore.staleAgents, workStateFor(agent, $heartbeatStore.agents[agent.id]));
  $: reasonLabel = reason === 'delivery_failed' ? 'Delivery failed' : reason === 'error' ? 'Agent error' : 'Stale heartbeat';
  $: dependencies = byAgent.get(agent.id);
  $: statusLabel = serdeEnumVariantName(agent.status) || 'Unknown status';
</script>

<div class="tree-item">
  <div
    class="tree-row"
    class:selected={isSelected}
    role="treeitem"
    tabindex="0"
    data-agent-id={agent.id}
    aria-level={depth + 1}
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
      <span class="identity"><span class="label">{displayLabel}</span><span class="status-label">{statusLabel}</span></span>
      <AgentActivityLine agentId={agent.id} compact />
      {#each ['waitingOn', 'blocks'] as kind}
        {@const refs = kind === 'waitingOn' ? dependencies?.waitingOn : dependencies?.blocks}
        {#if refs?.length}
          <span class="dependencies">
            <span class="dependency-label">{kind === 'waitingOn' ? 'Waiting on' : 'Blocks'}{refs.length > 1 ? ` (${refs.length})` : ''}:</span>
            {#each refs as ref (ref.nodeId)}
              {#if ref.agentId && sessionAgentIds.has(ref.agentId)}
                <button type="button" class="dependency-ref" title={ref.title}
                  aria-label={`Focus agent for ${ref.title}`}
                  on:click={(event) => { event.stopPropagation(); dispatch('select', ref.agentId!); }}
                >{ref.title}</button>
              {:else}
                <span class="dependency-text" title={ref.title}>{ref.title}</span>
              {/if}
            {/each}
          </span>
        {/if}
      {/each}
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
          {sessionAgentIds}
          depth={depth + 1}
          {selectedId}
          {byAgent}
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
    transition: background-color var(--motion-ui);
  }

  .tree-row:hover {
    background: var(--surface-raised);
  }

  .tree-row.selected {
    background: var(--accent-tint);
    color: var(--accent);
  }

  .tree-row:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: -2px;
  }

  .indent {
    flex-shrink: 0;
  }

  .chevron-spacer {
    width: 16px;
    flex-shrink: 0;
  }

  .agent-copy { display: flex; flex: 1; flex-direction: column; min-width: 0; }
  .identity { display: flex; align-items: baseline; gap: 6px; min-width: 0; }
  .status-label { color: var(--text-muted); font-size: 10px; flex-shrink: 0; }
  .dependencies { display: flex; flex-wrap: wrap; align-items: baseline; gap: 3px 5px; margin-top: 3px; font-size: 11px; }
  .dependency-label { color: var(--text-muted); }
  .dependency-ref, .dependency-text { max-width: 100%; overflow-wrap: anywhere; }
  .dependency-ref { font: inherit; color: var(--text-primary); border: 1px solid var(--border-control); border-radius: var(--control-radius); padding: 1px 5px; background: var(--surface); cursor: pointer; text-align: left; }
  .dependency-ref:hover { text-decoration: underline; }
  .dependency-ref:focus-visible { outline: 2px solid var(--focus); outline-offset: 1px; }

  .label {
    font-size: 13px;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .tree-row.selected .label { color: var(--accent); }

  .attention-badge { font-size: 10px; padding: 2px 5px; min-height: 22px; flex-shrink: 0; }

  .status-dot { width: 7px; height: 7px; border: 1px solid currentColor; border-radius: 50%; background: currentColor; flex-shrink: 0; }

  @media (forced-colors: active) {
    .status-dot { background: CanvasText; border-color: CanvasText; }
    .attention-badge { border: 1px solid ButtonText; }
    .dependency-ref { color: ButtonText; border-color: ButtonText; background: ButtonFace; }
    .tree-row:focus-visible, .dependency-ref:focus-visible { outline: 2px solid Highlight; box-shadow: none; }
    .tree-row.selected { outline: 1px solid Highlight; outline-offset: -1px; }
  }

  .children {
    display: flex;
    flex-direction: column;
  }
</style>
