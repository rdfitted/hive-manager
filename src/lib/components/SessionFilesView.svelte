<script lang="ts">
  import { onMount } from 'svelte';
  import { ArrowClockwise, CaretDown, CaretRight, FileText, Folder } from 'phosphor-svelte';
  import { activeSession } from '$lib/stores/sessions';
  import {
    SESSION_FILES_POLL_INTERVAL,
    sessionFilesStore,
    type SessionFileEntry,
  } from '$lib/stores/sessionFiles';

  const FILE_WINDOW_SIZE = 200;

  let collapsedDirectories = $state(new Set<string>());
  let windowStart = $state(0);
  let observedSessionId: string | null = null;

  const visibleEntries = $derived(
    $sessionFilesStore.entries.filter((entry) => isVisible(entry.path, collapsedDirectories)),
  );
  const maxWindowStart = $derived(
    visibleEntries.length === 0
      ? 0
      : Math.floor((visibleEntries.length - 1) / FILE_WINDOW_SIZE) * FILE_WINDOW_SIZE,
  );
  const effectiveWindowStart = $derived(Math.min(windowStart, maxWindowStart));
  const windowEntries = $derived(
    visibleEntries.slice(effectiveWindowStart, effectiveWindowStart + FILE_WINDOW_SIZE),
  );
  const windowEnd = $derived(effectiveWindowStart + windowEntries.length);
  const selectedEntry = $derived(
    $sessionFilesStore.selectedPath
      ? $sessionFilesStore.entries.find(
          (entry) => entry.path === $sessionFilesStore.selectedPath,
        ) ?? null
      : null,
  );

  $effect(() => {
    const sessionId = $activeSession?.id ?? null;
    if (sessionId === observedSessionId) return;

    observedSessionId = sessionId;
    collapsedDirectories = new Set();
    windowStart = 0;
    sessionFilesStore.setSessionId(sessionId);
    if (sessionId) void sessionFilesStore.loadFiles(sessionId);
  });

  $effect(() => {
    if (windowStart > maxWindowStart) windowStart = maxWindowStart;
  });

  onMount(() => {
    const pollInterval = setInterval(() => {
      void sessionFilesStore.pollFiles();
    }, SESSION_FILES_POLL_INTERVAL);

    return () => clearInterval(pollInterval);
  });

  function normalizePath(path: string): string {
    return path.replaceAll('\\', '/').replace(/^\.\//, '');
  }

  function pathParts(path: string): string[] {
    return normalizePath(path)
      .split('/')
      .filter((part) => part.length > 0 && part !== '.');
  }

  function entryDepth(path: string): number {
    return Math.max(0, pathParts(path).length - 1);
  }

  function isCollapsed(path: string): boolean {
    return collapsedDirectories.has(normalizePath(path));
  }

  function isVisible(path: string, collapsed: Set<string>): boolean {
    const parts = pathParts(path);
    for (let index = 1; index < parts.length; index += 1) {
      if (collapsed.has(parts.slice(0, index).join('/'))) return false;
    }
    return true;
  }

  function toggleDirectory(path: string): void {
    const normalized = normalizePath(path);
    const next = new Set(collapsedDirectories);
    if (next.has(normalized)) next.delete(normalized);
    else next.add(normalized);
    collapsedDirectories = next;
    windowStart = 0;
  }

  function showPreviousWindow(): void {
    windowStart = Math.max(0, effectiveWindowStart - FILE_WINDOW_SIZE);
  }

  function showNextWindow(): void {
    windowStart = Math.min(maxWindowStart, effectiveWindowStart + FILE_WINDOW_SIZE);
  }

  function handleEntryClick(entry: SessionFileEntry): void {
    if (entry.is_dir) {
      toggleDirectory(entry.path);
      return;
    }

    void sessionFilesStore.selectFile(entry.path);
  }

  function formatSize(size: number): string {
    if (!Number.isFinite(size) || size < 0) return 'Unknown size';
    if (size < 1024) return `${size} B`;
    if (size < 1024 * 1024) return `${(size / 1024).toFixed(size < 10 * 1024 ? 1 : 0)} KB`;
    return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  }

  function formatModified(modified: SessionFileEntry['modified']): string {
    if (modified === null || modified === '') return 'Modified time unavailable';
    const value = typeof modified === 'number' && modified < 1_000_000_000_000
      ? modified * 1000
      : modified;
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? 'Modified time unavailable' : date.toLocaleString();
  }
</script>

<div class="session-files-view">
  {#if !$activeSession}
    <div class="empty-state" aria-live="polite">
      <Folder size={44} weight="light" />
      <p>No active session</p>
      <span>Select a session to browse its files.</span>
    </div>
  {:else}
    <header class="files-header">
      <div class="heading">
        <span class="title">Session Files</span>
        <span class="read-only-badge">Read only</span>
      </div>
      <button
        type="button"
        class="refresh-button"
        class:spinning={$sessionFilesStore.loading || $sessionFilesStore.refreshing}
        onclick={() => void sessionFilesStore.refresh()}
        disabled={$sessionFilesStore.loading || $sessionFilesStore.refreshing}
        title="Refresh session files"
        aria-label="Refresh session files"
      >
        <ArrowClockwise size={15} weight="light" />
      </button>
    </header>

    {#if $sessionFilesStore.error && $sessionFilesStore.entries.length > 0}
      <div class="inline-error" role="alert">
        <span>{$sessionFilesStore.error}</span>
        <button type="button" onclick={() => sessionFilesStore.clearError()}>Dismiss</button>
      </div>
    {/if}

    <section class="file-browser" aria-label="Session file browser">
      {#if $sessionFilesStore.loading}
        <div class="pane-state" aria-live="polite">
          <span class="spinner" aria-hidden="true"></span>
          <span>Loading session files…</span>
        </div>
      {:else if $sessionFilesStore.error && $sessionFilesStore.entries.length === 0}
        <div class="pane-state error-state" role="alert">
          <span>{$sessionFilesStore.error}</span>
          <button type="button" onclick={() => void sessionFilesStore.refresh()}>Try again</button>
        </div>
      {:else if $sessionFilesStore.entries.length === 0}
        <div class="pane-state" aria-live="polite">
          <Folder size={30} weight="light" />
          <span>No session files yet.</span>
          <small>This list refreshes automatically.</small>
        </div>
      {:else}
        <div id="session-files-tree" class="file-list" role="tree" aria-label="Files">
          {#each windowEntries as entry (entry.path)}
            {@const selected = !entry.is_dir && $sessionFilesStore.selectedPath === entry.path}
            <button
              type="button"
              class="file-row"
              class:selected
              role="treeitem"
              aria-selected={selected}
              aria-expanded={entry.is_dir ? !isCollapsed(entry.path) : undefined}
              style={`padding-left: ${8 + entryDepth(entry.path) * 14}px`}
              title={`${normalizePath(entry.path)}\n${entry.is_dir ? 'Folder' : formatSize(entry.size)}\n${formatModified(entry.modified)}`}
              onclick={() => handleEntryClick(entry)}
            >
              <span class="disclosure" aria-hidden="true">
                {#if entry.is_dir}
                  {#if isCollapsed(entry.path)}
                    <CaretRight size={12} weight="bold" />
                  {:else}
                    <CaretDown size={12} weight="bold" />
                  {/if}
                {/if}
              </span>
              {#if entry.is_dir}
                <span class="entry-icon folder-icon">
                  <Folder size={15} weight="light" />
                </span>
              {:else}
                <span class="entry-icon">
                  <FileText size={15} weight="light" />
                </span>
              {/if}
              <span class="entry-name">{entry.name}</span>
              {#if !entry.is_dir}
                <span class="entry-size">{formatSize(entry.size)}</span>
              {/if}
            </button>
          {/each}
        </div>
        {#if visibleEntries.length > FILE_WINDOW_SIZE}
          <div class="file-window-controls" role="group" aria-label="File list navigation">
            <button
              type="button"
              aria-controls="session-files-tree"
              onclick={showPreviousWindow}
              disabled={effectiveWindowStart === 0}
            >
              Previous files
            </button>
            <span class="file-window-status" aria-live="polite">
              Showing {effectiveWindowStart + 1}–{windowEnd} of {visibleEntries.length}
            </span>
            <button
              type="button"
              aria-controls="session-files-tree"
              onclick={showNextWindow}
              disabled={windowEnd >= visibleEntries.length}
            >
              Next files
            </button>
          </div>
        {/if}
      {/if}
    </section>

    <section class="content-viewer" aria-label="File content">
      {#if selectedEntry}
        <header class="content-header">
          <div class="content-heading">
            <FileText size={14} weight="light" />
            <span title={normalizePath(selectedEntry.path)}>{normalizePath(selectedEntry.path)}</span>
          </div>
          <span class="content-size">{formatSize(selectedEntry.size)}</span>
        </header>
      {/if}

      <div class="content-body">
        {#if $sessionFilesStore.contentLoading}
          <div class="pane-state" aria-live="polite">
            <span class="spinner" aria-hidden="true"></span>
            <span>Loading file…</span>
          </div>
        {:else if $sessionFilesStore.contentError}
          <div class="pane-state error-state" role="alert">
            <span>{$sessionFilesStore.contentError}</span>
            {#if $sessionFilesStore.selectedPath}
              <button
                type="button"
                onclick={() => void sessionFilesStore.selectFile($sessionFilesStore.selectedPath!)}
              >
                Try again
              </button>
            {/if}
          </div>
        {:else if $sessionFilesStore.content}
          <pre class="file-content">{$sessionFilesStore.content.content}</pre>
        {:else}
          <div class="pane-state content-empty" aria-live="polite">
            <FileText size={30} weight="light" />
            <span>Select a file to view its contents.</span>
          </div>
        {/if}
      </div>
    </section>
  {/if}
</div>

<style>
  .session-files-view {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg-void);
    color: var(--text-primary);
  }

  .files-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 9px 10px;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border-structural);
  }

  .heading,
  .content-heading {
    display: flex;
    align-items: center;
    min-width: 0;
    gap: 7px;
  }

  .title {
    font-size: 12px;
    font-weight: 600;
  }

  .read-only-badge {
    padding: 2px 5px;
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .refresh-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 27px;
    height: 27px;
    padding: 0;
    flex-shrink: 0;
    color: var(--text-secondary);
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .refresh-button:hover:not(:disabled) {
    color: var(--accent-cyan);
    background: var(--bg-elevated);
    border-color: var(--border-structural);
  }

  .refresh-button:disabled {
    opacity: 0.65;
    cursor: default;
  }

  .refresh-button.spinning :global(svg) {
    animation: spin 0.8s linear infinite;
  }

  .inline-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 7px 10px;
    color: var(--status-error);
    background: var(--bg-surface);
    border-bottom: 1px solid var(--status-error);
    font-size: 11px;
  }

  .inline-error span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .inline-error button,
  .error-state button {
    padding: 4px 8px;
    color: var(--text-primary);
    background: var(--bg-elevated);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    font-size: 10px;
    cursor: pointer;
  }

  .file-browser {
    min-height: 120px;
    max-height: 42%;
    flex: 0 1 42%;
    overflow: auto;
    border-bottom: 1px solid var(--border-structural);
  }

  .file-list {
    min-width: max-content;
    padding: 4px 0;
  }

  .file-window-controls {
    position: sticky;
    bottom: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-width: max-content;
    padding: 6px 8px;
    gap: 8px;
    background: var(--bg-surface);
    border-top: 1px solid var(--border-structural);
  }

  .file-window-controls button {
    padding: 4px 8px;
    color: var(--text-secondary);
    background: var(--bg-elevated);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    font-size: 10px;
    cursor: pointer;
  }

  .file-window-controls button:hover:not(:disabled) {
    color: var(--accent-cyan);
  }

  .file-window-controls button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .file-window-status {
    color: var(--text-muted);
    font-size: 10px;
    white-space: nowrap;
  }

  .file-row {
    display: flex;
    align-items: center;
    width: 100%;
    min-width: 100%;
    height: 28px;
    padding-top: 0;
    padding-right: 8px;
    padding-bottom: 0;
    gap: 5px;
    color: var(--text-secondary);
    background: none;
    border: none;
    font-family: var(--font-mono);
    font-size: 11px;
    text-align: left;
    cursor: pointer;
  }

  .file-row:hover {
    color: var(--text-primary);
    background: var(--bg-elevated);
  }

  .file-row.selected {
    color: var(--accent-cyan);
    background: var(--bg-elevated);
  }

  .file-row:focus-visible,
  .refresh-button:focus-visible,
  .file-window-controls button:focus-visible,
  .inline-error button:focus-visible,
  .error-state button:focus-visible {
    outline: 1px solid var(--accent-cyan);
    outline-offset: -1px;
  }

  .disclosure {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 12px;
    flex: 0 0 12px;
  }

  .entry-icon {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
  }

  .folder-icon {
    color: var(--accent-amber);
  }

  .entry-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .entry-size {
    margin-left: auto;
    padding-left: 12px;
    color: var(--text-muted);
    font-size: 10px;
    white-space: nowrap;
  }

  .content-viewer {
    display: flex;
    flex: 1;
    min-height: 0;
    flex-direction: column;
  }

  .content-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border-structural);
    font-family: var(--font-mono);
    font-size: 10px;
  }

  .content-heading span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .content-size {
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .content-body {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .file-content {
    width: 100%;
    height: 100%;
    box-sizing: border-box;
    margin: 0;
    padding: 12px;
    overflow: auto;
    color: var(--text-primary);
    background: var(--bg-void);
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    tab-size: 2;
  }

  .pane-state,
  .empty-state {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-direction: column;
    gap: 8px;
    min-height: 100%;
    padding: 24px 16px;
    box-sizing: border-box;
    color: var(--text-secondary);
    font-size: 12px;
    text-align: center;
  }

  .pane-state small,
  .empty-state span {
    color: var(--text-muted);
    font-size: 10px;
  }

  .content-empty {
    flex: 1;
  }

  .error-state {
    color: var(--status-error);
  }

  .empty-state {
    flex: 1;
  }

  .empty-state p {
    margin: 4px 0 0;
    color: var(--text-secondary);
    font-size: 13px;
  }

  .spinner {
    width: 14px;
    height: 14px;
    box-sizing: border-box;
    border: 2px solid var(--border-structural);
    border-top-color: var(--accent-cyan);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .refresh-button.spinning :global(svg),
    .spinner {
      animation: none;
    }
  }
</style>
