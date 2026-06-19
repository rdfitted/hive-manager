<script lang="ts" module>
  type TerminalSelectionReader = () => string | null;

  const terminalSelectionReaders = new Map<string, TerminalSelectionReader>();

  export function readTerminalSelection(agentId?: string | null): string | null {
    if (agentId) {
      return terminalSelectionReaders.get(agentId)?.() || null;
    }

    for (const read of terminalSelectionReaders.values()) {
      const selection = read();
      if (selection) return selection;
    }

    return null;
  }
</script>

<script lang="ts">
  import { ArrowDown, Broom, CaretDown, CaretUp, CheckSquare, ClipboardText, FileText, MagnifyingGlass, X } from 'phosphor-svelte';
  import { onMount, onDestroy, tick } from 'svelte';
  import { Terminal as XTerm } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { WebglAddon } from '@xterm/addon-webgl';
  import { SearchAddon } from '@xterm/addon-search';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { writeText, readText } from '@tauri-apps/plugin-clipboard-manager';
  import { activeAgents } from '$lib/stores/sessions';
  import { settings } from '$lib/stores/settings';
  import '@xterm/xterm/css/xterm.css';

  interface Props {
    agentId: string;
    isFocused?: boolean;
    onStatusChange?: (status: string) => void;
  }

  let { agentId, isFocused = false, onStatusChange }: Props = $props();

  let terminalContainer: HTMLDivElement;
  let term: XTerm | null = null;
  let fitAddon: FitAddon | null = null;
  let webglAddon: WebglAddon | null = null;
  let searchAddon: SearchAddon | null = null;
  let unlistenOutput: UnlistenFn | null = null;
  let unlistenStatus: UnlistenFn | null = null;
  let unlistenDragDrop: UnlistenFn | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let lastDims = { cols: 0, rows: 0 };
  let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
  let dragLeaveTimeout: ReturnType<typeof setTimeout> | null = null;
  let wasHidden = false; // Track if terminal was ever hidden to conditionally re-fit

  // Context menu state
  let showContextMenu = $state(false);
  let contextMenuX = $state(0);
  let contextMenuY = $state(0);
  let hasSelection = $state(false);
  let isDragActive = $state(false);
  let isWindows = $state(false);

  // Find bar state
  let showSearch = $state(false);
  let searchQuery = $state('');
  let searchInputEl = $state<HTMLInputElement | null>(null);
  let searchResultIndex = $state(-1);
  let searchResultCount = $state(0);

  const FONT_SIZE_MIN = 8;
  const FONT_SIZE_MAX = 28;
  const FONT_SIZE_DEFAULT = 14;

  // Track agent status from store
  let agent = $derived($activeAgents.find(a => a.id === agentId));
  let isWaiting = $derived(agent?.status && typeof agent.status === 'object' && 'WaitingForInput' in agent.status);

  $effect(() => {
    const currentAgentId = agentId;
    terminalSelectionReaders.set(currentAgentId, () => term?.getSelection() || null);

    return () => {
      terminalSelectionReaders.delete(currentAgentId);
    };
  });

  $effect(() => {
    if (isFocused && term) {
      term.focus();
      // Re-fit terminal after becoming visible to restore correct dimensions.
      // Only needed if terminal was previously hidden (visibility: hidden corrupted dimensions).
      // Use requestAnimationFrame to ensure DOM has updated before fitting.
      if (wasHidden && fitAddon) {
        requestAnimationFrame(() => {
          if (term && fitAddon) {
            try {
              fitAddon.fit();
              const dims = fitAddon.proposeDimensions();
              if (dims && dims.cols > 0 && dims.rows > 0) {
                if (dims.cols !== lastDims.cols || dims.rows !== lastDims.rows) {
                  lastDims = { cols: dims.cols, rows: dims.rows };
                  invoke('resize_pty', { id: agentId, cols: dims.cols, rows: dims.rows }).catch(console.error);
                }
              }
            } catch (e) {
              console.error('Failed to fit terminal on focus restore:', e);
            }
            term.scrollToBottom();
            wasHidden = false;
          }
        });
      }
    } else if (!isFocused) {
      // Mark terminal as potentially hidden when it loses focus
      // (it may have visibility: hidden applied by parent)
      wasHidden = true;
    }
  });

  // Tokyo Night theme colors
  const tokyoNightTheme = {
    background: '#1a1b26',
    foreground: '#c0caf5',
    cursor: '#c0caf5',
    cursorAccent: '#1a1b26',
    selection: '#33467c',
    black: '#15161e',
    red: '#f7768e',
    green: '#9ece6a',
    yellow: '#e0af68',
    blue: '#7aa2f7',
    magenta: '#bb9af7',
    cyan: '#7dcfff',
    white: '#a9b1d6',
    brightBlack: '#414868',
    brightRed: '#f7768e',
    brightGreen: '#9ece6a',
    brightYellow: '#e0af68',
    brightBlue: '#7aa2f7',
    brightMagenta: '#bb9af7',
    brightCyan: '#7dcfff',
    brightWhite: '#c0caf5',
  };

  // Flag to suppress paste events that the browser fires AFTER our Ctrl+V
  // handler already read the clipboard via Tauri API. Without this, xterm's
  // internal paste listener also fires onData → double send.
  let suppressPaste = false;

  // Single shared decoder with stream mode so multi-byte UTF-8 sequences
  // split across 4KB PTY chunks don't decode to replacement characters.
  const ptyDecoder = new TextDecoder('utf-8');

  // Centralised guard for term.write(). @xterm/addon-webgl@0.19.0 can throw
  // from write() on long-running buffers, corrupting the renderer so the pane
  // collapses to only xterm's fallback glyph. On first failure, dispose the
  // WebGL addon and retry once; subsequent writes use the default DOM renderer.
  // All write paths (PTY listener, exported write()) must route through here.
  function writeSafely(data: string) {
    if (!term) return;
    try {
      term.write(data);
    } catch (e) {
      if (webglAddon) {
        console.error('xterm write failed, disposing WebGL addon and falling back to DOM renderer:', e);
        try { webglAddon.dispose(); } catch { /* ignore */ }
        webglAddon = null;
        try { term.write(data); } catch (e2) {
          console.error('xterm write failed after WebGL fallback:', e2);
        }
      } else {
        console.error('xterm write failed:', e);
      }
    }
  }

  async function sendToPty(data: string) {
    try {
      await invoke('write_to_pty', { id: agentId, data });
    } catch (err) {
      console.error('[Terminal] Failed to write to PTY:', err);
    }
  }

  async function pasteToPty(data: string) {
    try {
      await invoke('paste_to_pty', { id: agentId, data });
    } catch (err) {
      console.error('[Terminal] Failed to paste to PTY:', err);
    }
  }

  function shellEscapePath(path: string): string {
    if (isWindows) {
      return `"${path.replace(/"/g, '`"')}"`;
    }

    if (!path.includes("'")) {
      return `'${path}'`;
    }

    return `"${path.replace(/(["`$\\])/g, '\\$1')}"`;
  }

  function isPointerInsideTerminal(position: { x: number; y: number }): boolean {
    if (!terminalContainer) return false;

    const rect = terminalContainer.getBoundingClientRect();
    const scale = window.devicePixelRatio || 1;
    const left = rect.left * scale;
    const right = rect.right * scale;
    const top = rect.top * scale;
    const bottom = rect.bottom * scale;

    return position.x >= left && position.x <= right && position.y >= top && position.y <= bottom;
  }

  function setDragActive(active: boolean) {
    if (dragLeaveTimeout) {
      clearTimeout(dragLeaveTimeout);
      dragLeaveTimeout = null;
    }

    isDragActive = active;
  }

  function handleDomDragOver(event: DragEvent) {
    event.preventDefault();
    event.dataTransfer!.dropEffect = 'copy';
  }

  function handleDomDragEnter(event: DragEvent) {
    event.preventDefault();
  }

  function handleDomDragLeave(event: DragEvent) {
    event.preventDefault();

    if (event.currentTarget === event.target) {
      dragLeaveTimeout = setTimeout(() => {
        isDragActive = false;
      }, 30);
    }
  }

  function handleDomDrop(event: DragEvent) {
    event.preventDefault();
  }

  async function handleDroppedPaths(paths: string[]) {
    if (paths.length === 0) return;

    const payload = paths.map(shellEscapePath).join('\n');
    await pasteToPty(payload);
    term?.focus();
  }

  function handleQuickAction(action: string) {
    sendToPty(action + '\n');
    term?.focus();
  }

  // Handle resize. Guard: skip fit if terminal is hidden (visibility: hidden returns
  // 0x0 dimensions, corrupting xterm's scroll state) unless force-fitting.
  function handleResize(forceFit = false) {
    if (fitAddon && term) {
      if (!forceFit) {
        const rect = terminalContainer.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) {
          // Terminal is hidden, skip fit to prevent corruption
          return;
        }
      }
      try {
        fitAddon.fit();
        const dims = fitAddon.proposeDimensions();
        if (dims && dims.cols > 0 && dims.rows > 0) {
          if (dims.cols !== lastDims.cols || dims.rows !== lastDims.rows) {
            lastDims = { cols: dims.cols, rows: dims.rows };
            invoke('resize_pty', { id: agentId, cols: dims.cols, rows: dims.rows }).catch(console.error);
          }
        }
      } catch (e) {
        console.error('Failed to fit terminal:', e);
      }
    }
  }

  // Live-apply persisted font preferences to the running terminal.
  $effect(() => {
    const { fontSize, fontFamily } = $settings;
    if (term && (term.options.fontSize !== fontSize || term.options.fontFamily !== fontFamily)) {
      term.options.fontSize = fontSize;
      term.options.fontFamily = fontFamily;
      handleResize();
    }
  });

  function adjustFontSize(delta: number) {
    const current = $settings.fontSize ?? FONT_SIZE_DEFAULT;
    const next = Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, current + delta));
    settings.update({ fontSize: next });
  }

  function resetFontSize() {
    settings.update({ fontSize: FONT_SIZE_DEFAULT });
  }

  // ── Find-in-terminal (SearchAddon) ────────────────────────────────
  const searchDecorations = {
    matchBackground: '#33467c',
    matchOverviewRuler: '#7aa2f7',
    activeMatchBackground: '#e0af68',
    activeMatchColorOverviewRuler: '#e0af68',
  };

  async function openSearch() {
    showSearch = true;
    closeContextMenu();
    await tick();
    searchInputEl?.focus();
    searchInputEl?.select();
  }

  function closeSearch() {
    showSearch = false;
    searchResultIndex = -1;
    searchResultCount = 0;
    searchAddon?.clearDecorations();
    term?.focus();
  }

  function runSearch(direction: 'next' | 'previous', incremental = false) {
    if (!searchAddon || !searchQuery) {
      searchAddon?.clearDecorations();
      searchResultIndex = -1;
      searchResultCount = 0;
      return;
    }
    const options = { incremental, decorations: searchDecorations };
    if (direction === 'next') {
      searchAddon.findNext(searchQuery, options);
    } else {
      searchAddon.findPrevious(searchQuery, options);
    }
  }

  function handleSearchKeydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === 'Enter') {
      event.preventDefault();
      runSearch(event.shiftKey ? 'previous' : 'next');
    } else if (event.key === 'Escape') {
      event.preventDefault();
      closeSearch();
    }
  }

  function handleClearTerminal() {
    term?.clear();
    closeContextMenu();
    term?.focus();
  }

  function handleScrollToBottom() {
    term?.scrollToBottom();
    closeContextMenu();
    term?.focus();
  }

  function handleContextMenu(event: MouseEvent) {
    event.preventDefault();
    hasSelection = !!(term?.getSelection());
    contextMenuX = event.clientX;
    contextMenuY = event.clientY;
    showContextMenu = true;
  }

  function closeContextMenu() {
    showContextMenu = false;
  }

  async function handleCopy() {
    const selection = term?.getSelection();
    if (selection) {
      try {
        await writeText(selection);
      } catch (err) {
        console.error('Copy failed:', err);
        // Fallback to browser API
        navigator.clipboard.writeText(selection).catch(console.error);
      }
    }
    closeContextMenu();
    term?.focus();
  }

  async function handlePaste() {
    let text: string | null = null;

    // Try Tauri API first
    try {
      text = await readText();
    } catch {
      // Tauri API failed
    }

    // Fallback to browser API if Tauri didn't work
    if (!text) {
      try {
        text = await navigator.clipboard.readText();
      } catch {
        // Browser API also failed
      }
    }

    if (text) {
      await pasteToPty(text);
    }

    closeContextMenu();
    term?.focus();
  }

  function handleSelectAll() {
    term?.selectAll();
    closeContextMenu();
  }

  function handleClearSelection() {
    term?.clearSelection();
    closeContextMenu();
    term?.focus();
  }

  // Close context menu when clicking elsewhere
  function handleGlobalClick() {
    if (showContextMenu) {
      showContextMenu = false;
    }
  }

  // Global paste handler for when terminal has focus but paste targets document
  // (e.g., paste from external tools like Wispr Flow that target the document body)
  // Redirects the paste into the terminal so xterm's onData handles it.
  function handleGlobalPaste(event: ClipboardEvent) {
    // Only handle if this terminal is focused
    if (!isFocused || !term) return;

    // If target is already inside our terminal, xterm handles it natively
    if (terminalContainer?.contains(event.target as Node)) return;

    // If xterm's internal textarea has focus, xterm will handle the paste
    // via onData — don't also send it here
    const xtermTextarea = terminalContainer?.querySelector('.xterm-helper-textarea');
    if (xtermTextarea && document.activeElement === xtermTextarea) return;

    const text = event.clipboardData?.getData('text');
    if (text) {
      event.preventDefault();
      pasteToPty(text);
    }
  }

  onMount(async () => {
    isWindows = navigator.platform.toLowerCase().startsWith('win');

    // Add global click listener
    document.addEventListener('click', handleGlobalClick);
    // Add global paste listener for tools like Wispr Flow
    document.addEventListener('paste', handleGlobalPaste);

    unlistenDragDrop = await getCurrentWindow().onDragDropEvent(async (event) => {
      switch (event.payload.type) {
        case 'enter':
        case 'over':
          setDragActive(isPointerInsideTerminal(event.payload.position));
          break;
        case 'drop':
          if (isPointerInsideTerminal(event.payload.position)) {
            setDragActive(false);
            await handleDroppedPaths(event.payload.paths);
          } else {
            isDragActive = false;
          }
          break;
        case 'leave':
          isDragActive = false;
          break;
      }
    });

    // Create terminal instance
    term = new XTerm({
      theme: tokyoNightTheme,
      fontFamily: $settings.fontFamily,
      fontSize: $settings.fontSize,
      lineHeight: 1.2,
      cursorBlink: true,
      cursorStyle: 'block',
      scrollback: 10000,
      allowProposedApi: true,
    });

    // Load addons
    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);

    searchAddon = new SearchAddon();
    term.loadAddon(searchAddon);
    searchAddon.onDidChangeResults((results) => {
      searchResultIndex = results?.resultIndex ?? -1;
      searchResultCount = results?.resultCount ?? 0;
    });

    // Open terminal in container
    term.open(terminalContainer);

    // Capture-phase paste listener: suppresses paste events that the browser
    // fires after our Ctrl+V handler already sent clipboard content via Tauri API.
    // Must be capture phase to fire before xterm's own paste listener.
    terminalContainer.addEventListener('paste', (e) => {
      if (suppressPaste) {
        e.preventDefault();
        e.stopImmediatePropagation();
        suppressPaste = false;
      }
    }, true);

    // Try to load WebGL addon for better performance. Retain the reference so
    // a write-time rendering failure (see pty-output listener) can dispose it
    // and fall back to the default DOM renderer without losing the terminal.
    try {
      webglAddon = new WebglAddon();
      term.loadAddon(webglAddon);
    } catch (e) {
      webglAddon = null;
      console.warn('WebGL addon not supported, using canvas renderer');
    }

    // Fit to container
    fitAddon.fit();

    // Focus terminal immediately
    term.focus();

    // Custom key handler for special keys
    term.attachCustomKeyEventHandler((event) => {
      if (event.type !== 'keydown') return true;

      // Ctrl+F opens the find bar
      if (event.ctrlKey && !event.shiftKey && (event.key === 'f' || event.key === 'F')) {
        openSearch();
        return false;
      }

      // Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0 adjust terminal font size
      if (event.ctrlKey && (event.key === '=' || event.key === '+')) {
        adjustFontSize(1);
        return false;
      }
      if (event.ctrlKey && event.key === '-') {
        adjustFontSize(-1);
        return false;
      }
      if (event.ctrlKey && event.key === '0') {
        resetFontSize();
        return false;
      }

      // Shift+Enter inserts newline without submitting
      // Use bracketed paste mode so CLIs treat it as literal text, not command submission
      if (event.key === 'Enter' && event.shiftKey) {
        if (term) {
          sendToPty('\x1b[200~\n\x1b[201~');
        }
        return false;
      }

      // Ctrl+Shift+C or Ctrl+C with selection = Copy
      if (event.ctrlKey && (event.key === 'C' || (event.key === 'c' && event.shiftKey))) {
        const selection = term?.getSelection();
        if (selection) {
          writeText(selection).catch((err: unknown) => {
            console.error('Copy failed:', err);
            navigator.clipboard.writeText(selection).catch(console.error);
          });
          return false;
        }
        // If no selection, let Ctrl+C pass through as interrupt
        return true;
      }

      // Ctrl+Shift+V or Ctrl+V = Paste
      // Read clipboard explicitly via Tauri API (native paste events don't reliably
      // carry clipboardData in Tauri's webview). Suppress any browser paste event
      // that fires despite returning false, to prevent xterm double-sending.
      if (event.ctrlKey && (event.key === 'V' || event.key === 'v')) {
        suppressPaste = true;
        setTimeout(() => { suppressPaste = false; }, 500);
        (async () => {
          let text: string | null = null;
          try {
            text = await readText();
          } catch {
            // Tauri API failed, try browser fallback
          }
          if (!text) {
            try {
              text = await navigator.clipboard.readText();
            } catch {
              // Browser API also failed
            }
          }
          if (text && term) {
            pasteToPty(text);
          }
        })();
        return false;
      }

      // Ctrl+C without selection = send interrupt (let it pass through)
      if (event.ctrlKey && event.key === 'c' && !event.shiftKey) {
        const selection = term?.getSelection();
        if (selection) {
          writeText(selection).catch((err: unknown) => {
            console.error('Copy failed:', err);
            navigator.clipboard.writeText(selection).catch(console.error);
          });
          return false;
        }
        // No selection, pass through as interrupt signal
        return true;
      }

      return true; // Allow normal handling for other keys
    });

    // Handle terminal input
    term.onData(async (data) => {
      await sendToPty(data);
    });

    // Listen for PTY output. Uses writeSafely() so a WebGL renderer failure
    // doesn't blank the pane. Uses a shared streaming decoder so multi-byte
    // UTF-8 sequences split across chunks decode correctly.
    unlistenOutput = await listen<{ id: string; data: number[] }>('pty-output', (event) => {
      if (event.payload.id === agentId && term) {
        const text = ptyDecoder.decode(new Uint8Array(event.payload.data), { stream: true });
        writeSafely(text);
      }
    });

    // Listen for status changes
    unlistenStatus = await listen<{ id: string; status: string }>('pty-status', (event) => {
      if (event.payload.id === agentId && onStatusChange) {
        onStatusChange(event.payload.status);
      }
    });

    resizeObserver = new ResizeObserver(() => {
      if (resizeTimeout) clearTimeout(resizeTimeout);
      resizeTimeout = setTimeout(handleResize, 100);
    });
    resizeObserver.observe(terminalContainer);

    // Report initial size after container is likely ready
    requestAnimationFrame(() => {
      handleResize();
    });

  });

  onDestroy(() => {
    terminalSelectionReaders.delete(agentId);
    if (resizeTimeout) clearTimeout(resizeTimeout);
    if (dragLeaveTimeout) clearTimeout(dragLeaveTimeout);
    document.removeEventListener('click', handleGlobalClick);
    document.removeEventListener('paste', handleGlobalPaste);
    if (unlistenOutput) unlistenOutput();
    if (unlistenStatus) unlistenStatus();
    if (unlistenDragDrop) unlistenDragDrop();
    if (resizeObserver) resizeObserver.disconnect();
    if (term) term.dispose();
  });

  export function focus() {
    term?.focus();
  }

  export function write(data: string) {
    writeSafely(data);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
  class="terminal-wrapper"
  bind:this={terminalContainer}
  oncontextmenu={handleContextMenu}
  onclick={closeContextMenu}
  ondragenter={handleDomDragEnter}
  ondragover={handleDomDragOver}
  ondragleave={handleDomDragLeave}
  ondrop={handleDomDrop}
>
  {#if isDragActive}
    <div class="drop-overlay">
      <div class="drop-card">
        <span class="drop-title">Drop file here</span>
        <span class="drop-copy">Absolute path(s) will be pasted into the terminal.</span>
      </div>
    </div>
  {/if}

  {#if showSearch}
    <div class="search-bar" role="search">
      <span class="search-icon">
        <MagnifyingGlass size={14} weight="light" />
      </span>
      <input
        type="text"
        bind:this={searchInputEl}
        bind:value={searchQuery}
        oninput={() => runSearch('next', true)}
        onkeydown={handleSearchKeydown}
        placeholder="Find in terminal"
        aria-label="Find in terminal"
        spellcheck="false"
      />
      <span class="search-count" class:no-results={searchQuery !== '' && searchResultCount === 0}>
        {#if searchQuery !== ''}
          {searchResultCount > 0 ? `${searchResultIndex + 1}/${searchResultCount}` : 'No results'}
        {/if}
      </span>
      <button class="search-btn" onclick={() => runSearch('previous')} title="Previous match (Shift+Enter)" aria-label="Previous match" type="button">
        <CaretUp size={14} weight="light" />
      </button>
      <button class="search-btn" onclick={() => runSearch('next')} title="Next match (Enter)" aria-label="Next match" type="button">
        <CaretDown size={14} weight="light" />
      </button>
      <button class="search-btn" onclick={closeSearch} title="Close (Esc)" aria-label="Close find bar" type="button">
        <X size={14} weight="light" />
      </button>
    </div>
  {/if}

  {#if isWaiting}
    <div class="quick-response-overlay">
      <div class="quick-response-bar">
        <span class="prompt-text">Agent is waiting:</span>
        <button class="action-btn" onclick={() => handleQuickAction('y')}>y</button>
        <button class="action-btn" onclick={() => handleQuickAction('n')}>n</button>
        <button class="action-btn approve" onclick={() => handleQuickAction('approve')}>Approve</button>
        <button class="action-btn reject" onclick={() => handleQuickAction('reject')}>Reject</button>
      </div>
    </div>
  {/if}

  {#if showContextMenu}
    <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
    <div
      class="context-menu"
      style="left: {contextMenuX}px; top: {contextMenuY}px;"
      onclick={(e) => e.stopPropagation()}
    >
      <button class="context-item" onclick={handleCopy} disabled={!hasSelection}>
        <span class="context-icon">
          <ClipboardText size={14} weight="light" />
        </span>
        Copy
        <span class="context-shortcut">Ctrl+C</span>
      </button>
      <button class="context-item" onclick={handlePaste}>
        <span class="context-icon">
          <FileText size={14} weight="light" />
        </span>
        Paste
        <span class="context-shortcut">Ctrl+V</span>
      </button>
      <div class="context-divider"></div>
      <button class="context-item" onclick={openSearch}>
        <span class="context-icon">
          <MagnifyingGlass size={14} weight="light" />
        </span>
        Find
        <span class="context-shortcut">Ctrl+F</span>
      </button>
      <button class="context-item" onclick={handleScrollToBottom}>
        <span class="context-icon">
          <ArrowDown size={14} weight="light" />
        </span>
        Scroll to Bottom
      </button>
      <button class="context-item" onclick={handleClearTerminal}>
        <span class="context-icon">
          <Broom size={14} weight="light" />
        </span>
        Clear Terminal
      </button>
      <div class="context-divider"></div>
      <button class="context-item" onclick={handleSelectAll}>
        <span class="context-icon">
          <CheckSquare size={14} weight="light" />
        </span>
        Select All
        <span class="context-shortcut">Ctrl+A</span>
      </button>
      <button class="context-item" onclick={handleClearSelection} disabled={!hasSelection}>
        <span class="context-icon">
          <X size={14} weight="light" />
        </span>
        Clear Selection
      </button>
    </div>
  {/if}
</div>

<style>
  .terminal-wrapper {
    position: relative;
    width: 100%;
    height: 100%;
    background: var(--bg-void);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .terminal-wrapper :global(.xterm) {
    padding: 8px;
    height: 100%;
  }

  .terminal-wrapper :global(.xterm-viewport) {
    background: var(--bg-void) !important;
  }

  .drop-overlay {
    position: absolute;
    inset: 0;
    z-index: 12;
    display: flex;
    align-items: center;
    justify-content: center;
    background:
      linear-gradient(180deg, rgba(122, 162, 247, 0.14), rgba(122, 162, 247, 0.08)),
      rgba(26, 27, 38, 0.72);
    border: 2px dashed rgba(125, 207, 255, 0.9);
    pointer-events: none;
  }

  .drop-card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 16px 20px;
    min-width: 240px;
    border-radius: var(--radius-sm);
    background: rgba(36, 40, 59, 0.92);
    border: 1px solid rgba(125, 207, 255, 0.35);
    box-shadow: 0 14px 30px rgba(0, 0, 0, 0.28);
    text-align: center;
  }

  .drop-title {
    color: var(--accent-cyan);
    font-size: 13px;
    font-weight: 700;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .drop-copy {
    color: var(--text-secondary);
    font-size: 12px;
    line-height: 1.4;
  }

  .search-bar {
    position: absolute;
    top: 8px;
    right: 16px;
    z-index: 15;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 6px;
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  }

  .search-icon {
    display: flex;
    align-items: center;
    color: var(--text-secondary);
    padding-left: 2px;
  }

  .search-bar input {
    width: 180px;
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-size: 12px;
    padding: 4px 8px;
    outline: none;
  }

  .search-bar input:focus {
    border-color: var(--accent-cyan);
  }

  .search-count {
    min-width: 56px;
    text-align: center;
    font-size: 11px;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .search-count.no-results {
    color: var(--status-error);
  }

  .search-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    background: none;
    border: none;
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    cursor: pointer;
  }

  .search-btn:hover {
    background: var(--bg-elevated);
    color: var(--text-primary);
  }

  .quick-response-overlay {
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    padding: 8px 12px;
    background: rgba(26, 27, 38, 0.85);
    backdrop-filter: blur(4px);
    border-top: 1px solid var(--status-warning);
    z-index: 10;
    display: flex;
    justify-content: center;
    animation: slideUp 0.2s ease-out;
  }

  @keyframes slideUp {
    from { transform: translateY(100%); }
    to { transform: translateY(0); }
  }

  .quick-response-bar {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .prompt-text {
    font-size: 11px;
    color: var(--status-warning);
    font-weight: 600;
    text-transform: uppercase;
    margin-right: 4px;
  }

  .action-btn {
    padding: 4px 12px;
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .action-btn:hover {
    background: var(--bg-elevated);
    border-color: var(--accent-cyan);
  }

  .action-btn.approve {
    background: color-mix(in srgb, var(--status-success) 15%, transparent);
    border-color: var(--status-success);
    color: var(--status-success);
  }

  .action-btn.approve:hover {
    background: color-mix(in srgb, var(--status-success) 25%, transparent);
  }

  .action-btn.reject {
    background: color-mix(in srgb, var(--status-error) 15%, transparent);
    border-color: var(--status-error);
    color: var(--status-error);
  }

  .action-btn.reject:hover {
    background: color-mix(in srgb, var(--status-error) 25%, transparent);
  }

  /* Context Menu */
  .context-menu {
    position: fixed;
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    padding: 4px;
    min-width: 180px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
    z-index: 1000;
  }

  .context-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-size: 13px;
    cursor: pointer;
    text-align: left;
  }

  .context-item:hover:not(:disabled) {
    background: var(--bg-elevated);
  }

  .context-item:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .context-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    flex-shrink: 0;
  }

  .context-shortcut {
    margin-left: auto;
    font-size: 11px;
    color: var(--text-secondary);
  }

  .context-divider {
    height: 1px;
    background: var(--border-structural);
    margin: 4px 8px;
  }
</style>
