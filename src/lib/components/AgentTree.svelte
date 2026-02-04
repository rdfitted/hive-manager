<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { AgentInfo } from '$lib/stores/sessions';
  import TreeItem from './TreeItem.svelte';

  export let agents: AgentInfo[] = [];
  export let selectedId: string | null = null;

  const dispatch = createEventDispatcher<{ select: string }>();

  // Build a map of parent_id -> children for efficient lookup
  function getChildrenMap(agents: AgentInfo[]): Map<string | null, AgentInfo[]> {
    const map = new Map<string | null, AgentInfo[]>();
    agents.forEach((agent) => {
      const parentId = agent.parent_id;
      const existing = map.get(parentId) || [];
      existing.push(agent);
      map.set(parentId, existing);
    });
    return map;
  }

  function handleSelect(e: CustomEvent<string>) {
    dispatch('select', e.detail);
  }

  $: childrenMap = getChildrenMap(agents);
  $: rootAgents = childrenMap.get(null) || [];
</script>

<div class="agent-tree" role="tree" aria-label="Agent hierarchy">
  {#if rootAgents.length === 0}
    <p class="empty">No agents</p>
  {:else}
    {#each rootAgents as agent (agent.id)}
      <TreeItem
        {agent}
        {childrenMap}
        depth={0}
        {selectedId}
        on:select={handleSelect}
      />
    {/each}
  {/if}
</div>

<style>
  .agent-tree {
    display: flex;
    flex-direction: column;
    padding: 4px 0;
  }

  .empty {
    margin: 0;
    padding: 12px;
    font-size: 12px;
    color: var(--color-text-muted);
    text-align: center;
  }
</style>
