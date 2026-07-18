<script lang="ts">
  import { routePage } from '$lib/knowledge/navigation';
  import {
    ArrowClockwise,
    Brain,
    GitBranch,
    House,
    Kanban,
    ListBullets,
    MagnifyingGlass,
  } from 'phosphor-svelte';
  import KnowledgeGraph from '$lib/components/knowledge/KnowledgeGraph.svelte';
  import KnowledgePagePreview from '$lib/components/knowledge/KnowledgePagePreview.svelte';
  import KnowledgeTable from '$lib/components/knowledge/KnowledgeTable.svelte';
  import {
    EDGE_COLORS,
    EDGE_LABELS,
    filterKnowledgeGraph,
    folderColor,
    folderKindLabel,
    isRelationshipFolder,
  } from '$lib/knowledge/graphUtils';
  import { KNOWLEDGE_EDGE_KINDS, type KnowledgeView } from '$lib/knowledge/types';
  import { knowledgeStore } from '$lib/stores/knowledge';

  interface FocusTarget {
    readonly isConnected: boolean;
    focus: (options?: FocusOptions) => void;
  }

  let sessionId = $derived($routePage.url.searchParams.get('session_id') || null);
  let query = $state('');
  let folder = $state('all');
  let view = $state<KnowledgeView>('graph');
  let previewReturnFocus = $state.raw<FocusTarget | null>(null);

  let folders = $derived.by(() => {
    // Must stay lowercase and exact — indexOf does not case-fold, and a
    // mismatch silently sorts the folder to the tail with no error.
    const preferredOrder = [
      'patterns',
      'practices',
      'research',
      'project',
      'clients',
      'partners',
      'vendors',
      'operations',
    ];
    return [...new Set($knowledgeStore.graph.nodes.map((node) => node.folder))].sort(
      (left, right) => {
        const leftIndex = preferredOrder.indexOf(left);
        const rightIndex = preferredOrder.indexOf(right);
        if (leftIndex !== -1 || rightIndex !== -1) {
          return (leftIndex === -1 ? preferredOrder.length : leftIndex) -
            (rightIndex === -1 ? preferredOrder.length : rightIndex);
        }
        return left.localeCompare(right);
      },
    );
  });
  let filteredGraph = $derived(filterKnowledgeGraph($knowledgeStore.graph, query, folder));

  $effect(() => {
    const currentSessionId = sessionId;
    knowledgeStore.loadGraph(currentSessionId);
  });

  function selectNode(id: string, trigger?: Element) {
    if (trigger && typeof (trigger as Element & { focus?: unknown }).focus === 'function') {
      previewReturnFocus = trigger as Element & FocusTarget;
    }
    return knowledgeStore.selectNode(id, sessionId);
  }

  function folderCount(name: string): number {
    return $knowledgeStore.graph.nodes.filter((node) => node.folder === name).length;
  }
</script>

<svelte:head><title>Knowledge · Hive Manager</title></svelte:head>

<div class="knowledge-page">
  <header class="page-header">
    <div class="identity">
      <div class="identity-mark" aria-hidden="true"><Brain size={20} weight="light" /></div>
      <div>
        <div class="eyebrow">Institutional memory</div>
        <h1>Knowledge Atlas</h1>
      </div>
    </div>

    <div class="corpus-stats" aria-label="Knowledge corpus totals">
      <div><strong>{$knowledgeStore.graph.nodes.length}</strong><span>Pages</span></div>
      <div><strong>{$knowledgeStore.graph.edges.length}</strong><span>Links</span></div>
      <div><strong>{folders.length}</strong><span>Folders</span></div>
    </div>

    <nav class="page-nav" aria-label="Main views">
      <a href="/" title="Session view"><House size={16} weight="light" /><span>Sessions</span></a>
      <a href="/dashboard" title="Dashboard"><Kanban size={16} weight="light" /><span>Dashboard</span></a>
    </nav>
  </header>

  <section class="toolbar" aria-label="Knowledge controls">
    <label class="search-field">
      <MagnifyingGlass size={15} weight="light" aria-hidden="true" />
      <span class="sr-only">Search knowledge</span>
      <input bind:value={query} type="search" placeholder="Search title, path, or folder…" />
      {#if query}<kbd>{filteredGraph.nodes.length}</kbd>{/if}
    </label>

    <label class="folder-field">
      <span class="sr-only">Filter by folder</span>
      <select bind:value={folder} aria-label="Filter by folder">
        <option value="all">All folders ({$knowledgeStore.graph.nodes.length})</option>
        {#each folders as name (name)}
          <option value={name}>{name} ({folderCount(name)})</option>
        {/each}
      </select>
    </label>

    <div class="view-switch" aria-label="Knowledge view">
      <button
        type="button"
        class:active={view === 'graph'}
        aria-pressed={view === 'graph'}
        onclick={() => view = 'graph'}
      >
        <GitBranch size={14} weight="light" />Graph
      </button>
      <button
        type="button"
        class:active={view === 'table'}
        aria-pressed={view === 'table'}
        onclick={() => view = 'table'}
      >
        <ListBullets size={14} weight="light" />Table
      </button>
    </div>

    <button
      type="button"
      class="refresh"
      class:spinning={$knowledgeStore.refreshing}
      onclick={() => knowledgeStore.loadGraph(sessionId)}
      disabled={$knowledgeStore.loading || $knowledgeStore.refreshing}
      aria-label="Refresh knowledge graph"
      title="Refresh knowledge graph"
    >
      <ArrowClockwise size={15} weight="light" />
    </button>
  </section>

  {#if $knowledgeStore.error && $knowledgeStore.graph.nodes.length > 0}
    <div class="notice error-notice" role="alert">
      Refresh failed: {$knowledgeStore.error}. Showing the last loaded graph.
    </div>
  {/if}
  {#if $knowledgeStore.graph.truncated}
    <div class="notice cap-notice">
      This atlas reached its safety cap. Refine the corpus to see pages beyond the current bounded scan.
    </div>
  {/if}

  <div class="workspace" class:preview-open={$knowledgeStore.selectedId !== null}>
    <main class="atlas-panel" aria-live="polite">
      {#if $knowledgeStore.loading && $knowledgeStore.graph.nodes.length === 0}
        <div class="full-state loading-state">
          <div class="radar" aria-hidden="true"><span></span></div>
          <strong>Mapping the corpus</strong>
          <p>Scanning the safe wiki folders and project knowledge…</p>
        </div>
      {:else if $knowledgeStore.error && $knowledgeStore.graph.nodes.length === 0}
        <div class="full-state error-state" role="alert">
          <Brain size={32} weight="light" />
          <strong>Knowledge map unavailable</strong>
          <p>{$knowledgeStore.error}</p>
          <button type="button" onclick={() => knowledgeStore.loadGraph(sessionId)}>Try again</button>
        </div>
      {:else if $knowledgeStore.graph.nodes.length === 0}
        <div class="full-state empty-state">
          <Brain size={32} weight="light" />
          <strong>No knowledge pages found</strong>
          <p>
            Add markdown under the wiki folders — patterns, practices, research, clients,
            partners, vendors, operations — or the project’s .ai-docs folder.
          </p>
          <button type="button" onclick={() => knowledgeStore.loadGraph(sessionId)}>Scan again</button>
        </div>
      {:else if filteredGraph.nodes.length === 0}
        <div class="full-state empty-state">
          <MagnifyingGlass size={30} weight="light" />
          <strong>No matching pages</strong>
          <p>Try a broader search or choose another folder.</p>
          <button type="button" onclick={() => { query = ''; folder = 'all'; }}>Clear filters</button>
        </div>
      {:else}
        <div class="view-frame">
          <div class="result-strip">
            <span>{filteredGraph.nodes.length} of {$knowledgeStore.graph.nodes.length} pages</span>
            <span>{filteredGraph.edges.length} visible relationships</span>
          </div>
          <div class="view-body">
            {#if view === 'graph'}
              <KnowledgeGraph
                nodes={filteredGraph.nodes}
                edges={filteredGraph.edges}
                selectedId={$knowledgeStore.selectedId}
                onSelect={selectNode}
              />
            {:else}
              <KnowledgeTable
                nodes={filteredGraph.nodes}
                selectedId={$knowledgeStore.selectedId}
                onSelect={selectNode}
              />
            {/if}
          </div>
          <footer class="legend">
            <div class="legend-group" aria-label="Folder colors and shapes">
              {#each folders as name (name)}
                <span>
                  <i
                    class:diamond={isRelationshipFolder(name)}
                    style:background={folderColor(name)}
                  ></i>
                  <span class="sr-only">{folderKindLabel(name)}:</span>{name}
                </span>
              {/each}
            </div>
            {#if view === 'graph'}
              <div class="legend-group" aria-label="Node shape key">
                <span><i class="shape-key diamond"></i>Diamond · relationship entity</span>
                <span><i class="shape-key circle"></i>Circle · operational knowledge</span>
              </div>
              <div class="legend-group edge-legend" aria-label="Relationship types">
                {#each KNOWLEDGE_EDGE_KINDS as kind (kind)}
                  <span><i style:background={EDGE_COLORS[kind]}></i>{EDGE_LABELS[kind]}</span>
                {/each}
              </div>
            {/if}
          </footer>
        </div>
      {/if}
    </main>

    {#if $knowledgeStore.selectedId}
      <KnowledgePagePreview
        selectedId={$knowledgeStore.selectedId}
        page={$knowledgeStore.page}
        loading={$knowledgeStore.pageLoading}
        error={$knowledgeStore.pageError}
        onClose={() => knowledgeStore.selectNode(null, sessionId)}
        onRetry={() => knowledgeStore.retryPage(sessionId)}
        returnFocus={previewReturnFocus}
      />
    {/if}
  </div>
</div>

<style>
  :global(*) { box-sizing: border-box; }
  :global(body) { overflow: hidden; }

  .knowledge-page {
    display: flex;
    flex-direction: column;
    width: 100vw;
    height: 100vh;
    overflow: hidden;
    background: var(--bg-void);
    color: var(--text-primary);
    font-family: var(--font-body);
  }

  .page-header {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    min-height: 72px;
    padding: 0 var(--space-5);
    border-bottom: 1px solid var(--border-structural);
    background:
      linear-gradient(90deg, color-mix(in srgb, var(--accent-cyan) 4%, transparent), transparent 34%),
      var(--bg-surface);
  }

  .identity {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  .identity-mark {
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    border: 1px solid var(--accent-cyan);
    color: var(--accent-cyan);
    background: color-mix(in srgb, var(--accent-cyan) 7%, transparent);
    box-shadow: inset 0 0 12px color-mix(in srgb, var(--accent-cyan) 5%, transparent);
  }

  .eyebrow {
    margin-bottom: 2px;
    color: var(--text-disabled);
    font: var(--text-micro) var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.12em;
  }

  h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: var(--text-h1);
    line-height: 1;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .corpus-stats {
    display: flex;
    align-items: stretch;
    border: 1px solid var(--border-structural);
    background: var(--bg-void);
  }

  .corpus-stats div {
    display: flex;
    align-items: baseline;
    gap: 5px;
    padding: 6px 11px;
    border-right: 1px solid var(--border-structural);
  }

  .corpus-stats div:last-child { border-right: 0; }
  .corpus-stats strong { color: var(--accent-cyan); font: 14px var(--font-mono); }
  .corpus-stats span { color: var(--text-disabled); font: 9px var(--font-mono); text-transform: uppercase; }

  .page-nav {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  .page-nav a {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 7px 9px;
    border: 1px solid transparent;
    color: var(--text-secondary);
    font: var(--text-small) var(--font-mono);
    text-decoration: none;
  }

  .page-nav a:hover {
    border-color: var(--border-structural);
    background: var(--bg-elevated);
    color: var(--accent-cyan);
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 52px;
    padding: var(--space-2) var(--space-4);
    border-bottom: 1px solid var(--border-structural);
    background: var(--bg-surface);
  }

  .search-field {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: min(430px, 42vw);
    height: 34px;
    padding: 0 var(--space-3);
    border: 1px solid var(--border-structural);
    background: var(--bg-void);
    color: var(--text-disabled);
  }

  .search-field:focus-within {
    border-color: var(--accent-cyan);
    color: var(--accent-cyan);
  }

  .search-field input {
    flex: 1;
    min-width: 0;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--text-primary);
    font: var(--text-small) var(--font-mono);
  }

  .search-field input::placeholder { color: var(--text-disabled); }
  .search-field kbd { color: var(--accent-cyan); font: var(--text-micro) var(--font-mono); }

  .folder-field select {
    height: 34px;
    min-width: 170px;
    padding: 0 28px 0 var(--space-3);
    border: 1px solid var(--border-structural);
    border-radius: 0;
    outline: 0;
    background: var(--bg-void);
    color: var(--text-secondary);
    font: var(--text-small) var(--font-mono);
    text-transform: capitalize;
  }

  .folder-field select:focus { border-color: var(--accent-cyan); }

  .view-switch {
    display: flex;
    margin-left: auto;
    border: 1px solid var(--border-structural);
  }

  .view-switch button,
  .refresh {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    height: 32px;
    padding: 0 var(--space-3);
    border: 0;
    border-right: 1px solid var(--border-structural);
    background: var(--bg-void);
    color: var(--text-secondary);
    font: var(--text-micro) var(--font-mono);
    text-transform: uppercase;
    cursor: pointer;
  }

  .view-switch button:last-child { border-right: 0; }
  .view-switch button:hover, .view-switch button.active { color: var(--accent-cyan); background: var(--bg-elevated); }
  .view-switch button.active { box-shadow: inset 0 -2px var(--accent-cyan); }

  .refresh {
    width: 34px;
    padding: 0;
    border: 1px solid var(--border-structural);
  }

  .refresh:hover:not(:disabled) { color: var(--accent-cyan); background: var(--bg-elevated); }
  .refresh:disabled { opacity: 0.45; cursor: wait; }
  .refresh.spinning :global(svg) { animation: spin 0.8s linear infinite; }

  .notice {
    padding: 5px var(--space-4);
    border-bottom: 1px solid var(--border-structural);
    color: var(--text-secondary);
    background: var(--bg-elevated);
    font: var(--text-micro) var(--font-mono);
  }

  .error-notice { color: var(--status-warning); }
  .cap-notice { color: var(--accent-amber); }

  .workspace {
    flex: 1;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    min-height: 0;
    overflow: hidden;
  }

  .workspace.preview-open { grid-template-columns: minmax(0, 1fr) minmax(330px, 390px); }

  .atlas-panel {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .view-frame {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
  }

  .result-strip {
    display: flex;
    justify-content: space-between;
    padding: 5px var(--space-3);
    border-bottom: 1px solid var(--border-structural);
    color: var(--text-disabled);
    background: var(--bg-void);
    font: 9px var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .view-body { flex: 1; min-height: 0; }

  .legend {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    min-height: 34px;
    padding: 0 var(--space-3);
    border-top: 1px solid var(--border-structural);
    background: var(--bg-surface);
    overflow-x: auto;
  }

  .legend-group {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    white-space: nowrap;
  }

  .legend-group span {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--text-secondary);
    font: 9px var(--font-mono);
    text-transform: uppercase;
  }

  .legend-group i { width: 6px; height: 6px; }
  .legend-group i.diamond { transform: rotate(45deg); }
  .shape-key { background: var(--text-secondary); }
  .shape-key.circle { border-radius: 50%; }
  .edge-legend i { width: 13px; height: 1px; }

  .full-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    padding: var(--space-5);
    color: var(--text-secondary);
    text-align: center;
    background:
      linear-gradient(var(--border-structural) 1px, transparent 1px),
      linear-gradient(90deg, var(--border-structural) 1px, transparent 1px);
    background-size: 28px 28px;
  }

  .full-state > * { position: relative; }
  .full-state strong { margin-top: var(--space-3); color: var(--text-primary); font: 18px var(--font-display); }
  .full-state p { max-width: 430px; margin: var(--space-2) 0 0; font-size: var(--text-small); }
  .full-state button {
    margin-top: var(--space-4);
    padding: var(--space-2) var(--space-4);
    border: 1px solid var(--accent-cyan);
    background: var(--bg-void);
    color: var(--accent-cyan);
    font: var(--text-small) var(--font-mono);
    cursor: pointer;
  }

  .error-state > :global(svg) { color: var(--status-error); }
  .empty-state > :global(svg) { color: var(--text-disabled); }

  .radar {
    position: relative;
    width: 48px;
    height: 48px;
    border: 1px solid var(--border-structural);
    border-radius: 50%;
    background: radial-gradient(circle, var(--accent-cyan) 0 2px, transparent 3px);
    overflow: hidden;
  }

  .radar::before, .radar::after { content: ''; position: absolute; background: var(--border-structural); }
  .radar::before { left: 50%; top: 0; width: 1px; height: 100%; }
  .radar::after { top: 50%; left: 0; height: 1px; width: 100%; }
  .radar span { position: absolute; inset: 50% 50% 0 0; background: linear-gradient(45deg, transparent, color-mix(in srgb, var(--accent-cyan) 28%, transparent)); transform-origin: 100% 0; animation: radar 1.5s linear infinite; }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  @keyframes spin { to { transform: rotate(360deg); } }
  @keyframes radar { to { transform: rotate(360deg); } }

  @media (max-width: 920px) {
    .page-header { grid-template-columns: 1fr auto; }
    .corpus-stats { display: none; }
    .workspace.preview-open { grid-template-columns: minmax(0, 1fr) minmax(300px, 42vw); }
    .edge-legend { display: none; }
  }

  @media (max-width: 700px) {
    .page-header { min-height: 62px; padding: 0 var(--space-3); }
    .page-nav span { display: none; }
    .identity-mark { display: none; }
    .toolbar { flex-wrap: wrap; min-height: auto; padding: var(--space-2); }
    .search-field { width: 100%; }
    .folder-field { flex: 1; }
    .folder-field select { width: 100%; min-width: 0; }
    .view-switch { margin-left: 0; }
    .workspace.preview-open { grid-template-columns: 1fr; }
    .workspace.preview-open .atlas-panel { display: none; }
    .legend { display: none; }
  }

  @media (prefers-reduced-motion: reduce) {
    .refresh.spinning :global(svg), .radar span { animation: none; }
  }
</style>
