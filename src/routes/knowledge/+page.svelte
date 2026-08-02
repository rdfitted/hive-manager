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
  import Skeleton from '$lib/components/Skeleton.svelte';
  import SkelBar from '$lib/components/SkelBar.svelte';
  import {
    describeOmission,
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
  let folder = $state<string | null>(null);
  let view = $state<KnowledgeView>('graph');
  let previewReturnFocus = $state.raw<FocusTarget | null>(null);

  // The backend discovers folders from the wiki root, so this list is open-ended.
  // `preferredOrder` is a *hint* for the familiar folders only: anything it does
  // not name still appears, sorted alphabetically after them, and is selectable.
  // Comparison is case-folded so a folder named `Field Notes` cannot be silently
  // shunted to the tail by a casing mismatch.
  const PREFERRED_FOLDER_ORDER = [
    'patterns',
    'practices',
    'research',
    '.project',
    'clients',
    'partners',
    'vendors',
    'operations',
    'root',
  ];

  let folders = $derived.by(() => {
    const rank = (name: string) => {
      const index = PREFERRED_FOLDER_ORDER.indexOf(name.trim().toLowerCase());
      return index === -1 ? PREFERRED_FOLDER_ORDER.length : index;
    };
    return [...new Set($knowledgeStore.graph.nodes.map((node) => node.folder))].sort(
      (left, right) => rank(left) - rank(right) || left.localeCompare(right),
    );
  });
  let filteredGraph = $derived(filterKnowledgeGraph($knowledgeStore.graph, query, folder));
  let omissions = $derived($knowledgeStore.graph.omissions ?? []);
  let initialLoading = $derived($knowledgeStore.loading && $knowledgeStore.graph.nodes.length === 0);

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
  <header class="page-header atlas-surface atlas-surface--strip lattice-forced-colors-boundary">
    <div class="identity">
      <div class="identity-mark atlas-surface atlas-surface--active lattice-forced-colors-boundary lattice-forced-colors-boundary--active" aria-hidden="true"><Brain size={20} weight="light" /></div>
      <div>
        <div class="eyebrow">Institutional memory</div>
        <h1>Knowledge Atlas</h1>
      </div>
    </div>

    <div class="corpus-stats atlas-surface lattice-forced-colors-boundary" aria-label="Knowledge corpus totals">
      <div><strong>{$knowledgeStore.graph.nodes.length}</strong><span>Pages</span></div>
      <div><strong>{$knowledgeStore.graph.edges.length}</strong><span>Links</span></div>
      <div><strong>{folders.length}</strong><span>Folders</span></div>
    </div>

    <nav class="page-nav" aria-label="Main views">
      <a href="/" class="lattice-btn lattice-btn--ghost lattice-btn--compact" title="Session view"><House size={16} weight="light" /><span>Sessions</span></a>
      <a href="/dashboard" class="lattice-btn lattice-btn--ghost lattice-btn--compact" title="Dashboard"><Kanban size={16} weight="light" /><span>Dashboard</span></a>
    </nav>
  </header>

  <section class="toolbar atlas-surface atlas-surface--strip lattice-forced-colors-boundary" aria-label="Knowledge controls">
    <label class="search-field atlas-surface lattice-forced-colors-boundary lattice-forced-colors-boundary--focus-within">
      <MagnifyingGlass size={15} weight="light" aria-hidden="true" />
      <span class="sr-only">Search knowledge</span>
      <input bind:value={query} type="search" placeholder="Search title, path, or folder…" />
      {#if query}<kbd>{filteredGraph.nodes.length}</kbd>{/if}
    </label>

    <label class="folder-field">
      <span class="sr-only">Filter by folder</span>
      <select class="lattice-input" bind:value={folder} aria-label="Filter by folder">
        <option value={null}>All folders ({$knowledgeStore.graph.nodes.length})</option>
        {#each folders as name (name)}
          <option value={name}>{name} ({folderCount(name)})</option>
        {/each}
      </select>
    </label>

    <div class="view-switch atlas-surface lattice-forced-colors-boundary lattice-forced-colors-boundary--focus-within" aria-label="Knowledge view">
      <button
        type="button"
        class={`lattice-tab${view === 'graph' ? ' lattice-tab--active' : ''}`}
        aria-pressed={view === 'graph'}
        onclick={() => view = 'graph'}
      >
        <GitBranch size={14} weight="light" />Graph
      </button>
      <button
        type="button"
        class={`lattice-tab${view === 'table' ? ' lattice-tab--active' : ''}`}
        aria-pressed={view === 'table'}
        onclick={() => view = 'table'}
      >
        <ListBullets size={14} weight="light" />Table
      </button>
    </div>

    <button
      type="button"
      class="refresh lattice-btn lattice-btn--ghost lattice-btn--icon"
      onclick={() => knowledgeStore.loadGraph(sessionId)}
      disabled={$knowledgeStore.loading || $knowledgeStore.refreshing}
      aria-label="Refresh knowledge graph"
      title="Refresh knowledge graph"
    >
      <span class="refresh-icon" class:lattice-motion-spinner={$knowledgeStore.refreshing}>
        <ArrowClockwise size={15} weight="light" />
      </span>
    </button>
  </section>

  {#if $knowledgeStore.error && $knowledgeStore.graph.nodes.length > 0}
    <div class="notice error-notice atlas-surface atlas-surface--strip atlas-surface--raised lattice-forced-colors-boundary" role="alert">
      Refresh failed: {$knowledgeStore.error}. Showing the last loaded graph.
    </div>
  {/if}
  {#if omissions.length > 0}
    <div class="notice cap-notice atlas-surface atlas-surface--strip atlas-surface--raised lattice-forced-colors-boundary">
      <strong>Not everything is shown.</strong>
      <ul>
        {#each omissions as omission (omission.reason)}
          <li>{describeOmission(omission)}</li>
        {/each}
      </ul>
    </div>
  {:else if $knowledgeStore.graph.truncated}
    <!-- Older backend: the boolean without the report behind it. -->
    <div class="notice cap-notice atlas-surface atlas-surface--strip atlas-surface--raised lattice-forced-colors-boundary">
      This atlas reached a scan limit, but this backend does not report which one.
    </div>
  {/if}

  <div class="workspace" class:preview-open={$knowledgeStore.selectedId !== null}>
    <main class="atlas-panel" aria-live="polite">
      {#snippet atlasSkeleton()}
        <div class="full-state atlas-loading-shape">
          <SkelBar width="48px" height="48px" radius="full" />
          <SkelBar width="13rem" height="1.25rem" radius="md" />
          <SkelBar width="min(26rem, 72%)" height="0.75rem" />
          <div class="atlas-loading-rows">
            <SkelBar width="100%" height="2.25rem" radius="md" />
            <SkelBar width="86%" height="2.25rem" radius="md" />
            <SkelBar width="94%" height="2.25rem" radius="md" />
          </div>
        </div>
      {/snippet}

      <Skeleton loading={initialLoading} skeleton={atlasSkeleton} class="atlas-skeleton">
        {#if $knowledgeStore.error && $knowledgeStore.graph.nodes.length === 0}
          <div class="full-state error-state" role="alert">
            <Brain size={32} weight="light" />
            <strong>Knowledge map unavailable</strong>
            <p>{$knowledgeStore.error}</p>
            <button class="state-action lattice-btn lattice-btn--primary" type="button" onclick={() => knowledgeStore.loadGraph(sessionId)}>Try again</button>
          </div>
        {:else if $knowledgeStore.graph.nodes.length === 0}
          <div class="full-state empty-state">
            <Brain size={32} weight="light" />
            <strong>No knowledge pages found</strong>
            <p>
              Add markdown anywhere under the wiki root — every top-level folder is scanned, plus
              loose pages at the root itself — or in the project’s .ai-docs folder.
            </p>
            <button class="state-action lattice-btn lattice-btn--primary" type="button" onclick={() => knowledgeStore.loadGraph(sessionId)}>Scan again</button>
          </div>
        {:else if filteredGraph.nodes.length === 0}
          <div class="full-state empty-state">
            <MagnifyingGlass size={30} weight="light" />
            <strong>No matching pages</strong>
            <p>Try a broader search or choose another folder.</p>
            <button class="state-action lattice-btn lattice-btn--primary" type="button" onclick={() => { query = ''; folder = null; }}>Clear filters</button>
          </div>
        {:else}
          <div class="view-frame">
            <div class="result-strip atlas-surface atlas-surface--strip atlas-surface--sunken lattice-forced-colors-boundary">
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
            <footer class="legend lattice-scroll-chrome atlas-surface atlas-surface--strip lattice-forced-colors-boundary">
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
      </Skeleton>
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

  /* One recipe for Atlas chrome and controls. Variants only select tokens; the
     surface declarations stay centralized so graph/table lenses cannot drift. */
  .atlas-surface {
    --atlas-surface-background: var(--bg-panel);
    --atlas-surface-elevation: var(--elev-1);
    --atlas-surface-edge: var(--edge-lip);
    --atlas-surface-radius: var(--radius-md);

    border-radius: var(--atlas-surface-radius);
    background: var(--atlas-surface-background);
    box-shadow: var(--atlas-surface-elevation), var(--atlas-surface-edge);
  }

  .atlas-surface--strip {
    --atlas-surface-background: var(--bg-chrome);
    --atlas-surface-elevation: var(--edge-seam);
    --atlas-surface-radius: var(--radius-none);
  }

  .atlas-surface--raised {
    --atlas-surface-background: var(--bg-raised);
  }

  .atlas-surface--sunken {
    --atlas-surface-background: var(--bg-void);
  }

  .atlas-surface--active {
    --atlas-surface-background: color-mix(in srgb, var(--accent-cyan) 5%, var(--bg-panel));
    --atlas-surface-elevation: inset 0 0 0 1px var(--accent-cyan), var(--elev-1);
  }

  .page-header {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    min-height: 72px;
    padding: 0 var(--space-5);
    --atlas-surface-background:
      linear-gradient(90deg, color-mix(in srgb, var(--accent-cyan) 4%, transparent), transparent 34%),
      var(--bg-chrome);
  }

  /* Full-bleed route chrome and its hover tones cannot adopt bordered panel primitives. */

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
    color: var(--accent-cyan);
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
    gap: 1px;
    padding: 1px;
  }

  .corpus-stats div {
    display: flex;
    align-items: baseline;
    gap: 5px;
    padding: 6px 11px;
    background: var(--bg-chrome);
  }

  .corpus-stats strong { color: var(--accent-cyan); font: 14px var(--font-mono); }
  .corpus-stats span { color: var(--text-disabled); font: 9px var(--font-mono); text-transform: uppercase; }

  .page-nav {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  .page-nav :global(.lattice-btn) {
    gap: 6px;
    text-decoration: none;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 52px;
    padding: var(--space-2) var(--space-4);
  }

  .search-field {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: min(430px, 42vw);
    height: 34px;
    padding: 0 var(--space-3);
    color: var(--text-disabled);
  }

  .search-field:focus-within {
    --atlas-surface-background: color-mix(in srgb, var(--accent-cyan) 5%, var(--bg-panel));
    --atlas-surface-elevation: inset 0 0 0 1px var(--accent-cyan), var(--elev-1);
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
    outline: 0;
    text-transform: capitalize;
  }

  .view-switch {
    display: flex;
    gap: var(--space-1);
    margin-left: auto;
    padding: 1px;
  }

  .refresh {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 32px;
    padding: 0;
  }

  .refresh-icon {
    display: inline-flex;
  }

  .notice {
    padding: 5px var(--space-4);
    color: var(--text-secondary);
    font: var(--text-micro) var(--font-mono);
  }

  .error-notice { color: var(--status-warning); }
  .cap-notice { color: var(--accent-amber); }
  .cap-notice strong { font-weight: 600; }
  .cap-notice ul {
    display: flex;
    flex-wrap: wrap;
    gap: 2px var(--space-3);
    margin: 2px 0 0;
    padding: 0;
    list-style: none;
  }
  .cap-notice li { color: var(--text-secondary); }
  .cap-notice li::before { content: '· '; color: var(--accent-amber); }

  .workspace {
    flex: 1;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr);
    min-height: 0;
    overflow: hidden;
  }

  .workspace.preview-open { grid-template-columns: minmax(0, 1fr) minmax(330px, 390px); }

  .atlas-panel {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .atlas-panel :global(.atlas-skeleton),
  .atlas-panel :global(.lattice-skeleton-content) {
    width: 100%;
    height: 100%;
  }

  .atlas-panel :global(.atlas-skeleton) {
    min-height: 0;
    grid-template-rows: minmax(0, 1fr);
  }

  .atlas-panel :global(.atlas-skeleton > .lattice-skeleton-placeholder),
  .atlas-panel :global(.atlas-skeleton > .lattice-skeleton-content) {
    min-height: 0;
    overflow: hidden;
  }

  .atlas-loading-shape {
    gap: var(--space-3);
  }

  .atlas-loading-rows {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    width: min(34rem, 78%);
    margin-top: var(--space-3);
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
    color: var(--text-disabled);
    font: 9px var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .view-body {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .legend {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    min-height: 34px;
    padding: 0 var(--space-3);
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
  .state-action {
    margin-top: var(--space-4);
  }

  .error-state > :global(svg) { color: var(--status-error); }
  .empty-state > :global(svg) { color: var(--text-disabled); }

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

</style>
