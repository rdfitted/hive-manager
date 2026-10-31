<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { AgentInfo } from '$lib/stores/sessions';
  import { agentDependencies } from '$lib/stores/agentDependencies';
  import TreeItem from './TreeItem.svelte';

  export let agents: AgentInfo[] = [];
  export let selectedId: string | null = null;

  const dispatch = createEventDispatcher<{ select: string }>();

  function handleNavigation(event: KeyboardEvent) {
    if (event.isComposing || event.altKey || event.ctrlKey || event.metaKey) return;
    const row = event.target as HTMLElement;
    if (row.getAttribute('role') !== 'treeitem') return;
    const tree = event.currentTarget as HTMLElement;
    const rows = Array.from(tree.querySelectorAll<HTMLElement>('[role="treeitem"]'));
    const index = rows.indexOf(row);
    const next = event.key === 'Home' ? rows[0] : event.key === 'End' ? rows.at(-1)
      : event.key === 'ArrowDown' ? rows[index + 1] : event.key === 'ArrowUp' ? rows[index - 1] : undefined;
    if (['Home', 'End', 'ArrowUp', 'ArrowDown'].includes(event.key)) {
      event.preventDefault();
      next?.focus();
    }
  }

  // Pick only visible rows. Session changes and collapsed/removed children must
  // leave a keyboard entry without changing terminal selection or DOM focus.
  function treeFocus(node: HTMLDivElement, selected: string | null) {
    let navigationId: string | null = null;
    function sync() {
      const rows = Array.from(node.querySelectorAll<HTMLElement>('[role="treeitem"]'));
      const target = rows.find((row) => row.dataset.agentId === navigationId)
        ?? rows.find((row) => row.dataset.agentId === selected) ?? rows[0];
      for (const row of rows) row.tabIndex = row === target ? 0 : -1;
    }
    function focus(event: FocusEvent) {
      navigationId = (event.target as HTMLElement).closest<HTMLElement>('[role="treeitem"]')?.dataset.agentId ?? navigationId;
      sync();
    }
    const observer = new MutationObserver(sync);
    observer.observe(node, { childList: true, subtree: true });
    node.addEventListener('focusin', focus);
    sync();
    return {
      update(value: string | null) { selected = value; sync(); },
      destroy() { observer.disconnect(); node.removeEventListener('focusin', focus); },
    };
  }

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

<div class="agent-tree lattice-scroll-chrome" role="tree" aria-label="Agent hierarchy" tabindex="-1"
  on:keydown={handleNavigation} use:treeFocus={selectedId}>
  {#if rootAgents.length === 0}
    <p class="empty">No agents</p>
  {:else}
    {#each rootAgents as agent (agent.id)}
      <TreeItem
        {agent}
        {childrenMap}
        depth={0}
        {selectedId}
        byAgent={$agentDependencies.byAgent}
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
    color: var(--text-secondary);
    text-align: center;
  }
</style>
