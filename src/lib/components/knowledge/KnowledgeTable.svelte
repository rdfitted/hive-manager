<script lang="ts">
  import { compareKnowledgeNodes, formatLastUpdated, folderColor, nodeDegree } from '$lib/knowledge/graphUtils';
  import type { KnowledgeNode, KnowledgeSortKey, SortDirection } from '$lib/knowledge/types';

  interface Props {
    nodes: KnowledgeNode[];
    selectedId: string | null;
    onSelect: (id: string, trigger?: Element) => void;
  }

  let { nodes, selectedId, onSelect }: Props = $props();
  let sortKey = $state<KnowledgeSortKey>('title');
  let sortDirection = $state<SortDirection>('asc');
  let sortedNodes = $derived(
    [...nodes].sort((left, right) => compareKnowledgeNodes(left, right, sortKey, sortDirection)),
  );

  const columns: { key: KnowledgeSortKey; label: string }[] = [
    { key: 'title', label: 'Title' },
    { key: 'folder', label: 'Folder' },
    { key: 'degree', label: 'Degree' },
    { key: 'last_updated', label: 'Last updated' },
  ];

  function setSort(key: KnowledgeSortKey) {
    if (sortKey === key) {
      sortDirection = sortDirection === 'asc' ? 'desc' : 'asc';
    } else {
      sortKey = key;
      sortDirection = key === 'last_updated' || key === 'degree' ? 'desc' : 'asc';
    }
  }

  function ariaSort(key: KnowledgeSortKey): 'ascending' | 'descending' | 'none' {
    if (sortKey !== key) return 'none';
    return sortDirection === 'asc' ? 'ascending' : 'descending';
  }

</script>

<div class="table-scroll">
  <table class="lattice-table knowledge-table">
    <thead>
      <tr>
        {#each columns as column (column.key)}
          <th aria-sort={ariaSort(column.key)}>
            <button type="button" onclick={() => setSort(column.key)}>
              {column.label}
              <span class="sort-mark" class:active={sortKey === column.key} aria-hidden="true">
                {sortKey === column.key ? (sortDirection === 'asc' ? '↑' : '↓') : '↕'}
              </span>
            </button>
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each sortedNodes as node (node.id)}
        <tr
          class:selected={node.id === selectedId}
        >
          <td>
            <button
              type="button"
              class="title"
              aria-label={`Open ${node.title}`}
              onclick={(event) => onSelect(node.id, event.currentTarget)}
            >
              {node.title}
            </button>
            <span class="path">{node.path}</span>
          </td>
          <td>
            <span class="folder-dot" style:background={folderColor(node.folder)}></span>
            <span class="folder-name">{node.folder}</span>
          </td>
          <td class="degree">
            <strong>{nodeDegree(node)}</strong>
            <span>{node.in_degree} in · {node.out_degree} out</span>
          </td>
          <td class="updated">{formatLastUpdated(node.last_updated)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .table-scroll {
    height: 100%;
    overflow: auto;
    background: var(--bg-void);
  }

  .knowledge-table {
    min-width: 680px;
  }

  th {
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--bg-surface);
  }

  th button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 0;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    letter-spacing: inherit;
    text-transform: inherit;
    cursor: pointer;
    text-align: left;
  }

  th button:hover {
    color: var(--accent-cyan);
  }

  .sort-mark {
    color: var(--text-disabled);
  }

  .sort-mark.active {
    color: var(--accent-cyan);
  }

  tbody tr.selected td {
    background: color-mix(in srgb, var(--accent-cyan) 7%, var(--bg-surface));
  }

  tbody tr.selected td:first-child {
    box-shadow: inset 2px 0 var(--accent-cyan);
  }

  td:first-child {
    width: 48%;
  }

  .title,
  .path,
  .degree span {
    display: block;
  }

  .title {
    width: 100%;
    padding: 0;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--text-primary);
    font-family: var(--font-body);
    font-size: var(--text-base);
    text-align: left;
    cursor: pointer;
  }

  .title:hover,
  .title:focus-visible {
    color: var(--accent-cyan);
  }

  .title:focus-visible {
    box-shadow: inset 2px 0 var(--accent-cyan);
  }

  .path,
  .degree span {
    margin-top: 2px;
    color: var(--text-disabled);
    font-size: var(--text-micro);
  }

  .folder-dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    margin-right: var(--space-2);
    box-shadow: 0 0 6px currentColor;
  }

  .folder-name {
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .degree strong {
    color: var(--text-primary);
    font-weight: 500;
  }

  .updated {
    white-space: nowrap;
    color: var(--text-secondary);
  }
</style>
