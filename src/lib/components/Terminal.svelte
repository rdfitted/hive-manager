<script lang="ts">
  import { CheckSquare, ClipboardText, FileText, X } from 'phosphor-svelte';
  import { onMount, onDestroy } from 'svelte';
  import { Terminal as XTerm } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { WebglAddon } from '@xterm/addon-webgl';
  import { SearchAddon } from '@xterm/addon-search';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { writeText, readText } from '@tauri-apps/plugin-clipboard-manager';
  import { activeAgents } from '$lib/stores/sessions';
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

  // Track agent status from store
  let agent = $derived($activeAgents.find(a => a.id === agentId));
  let isWaiting = $derived(agent?.status && typeof agent.status === 'object' && 'WaitingForInput' in agent.status);

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
      fontFamily: 'Cascadia Code, Consolas, monospace',
      fontSize: 14,
      lineHeight: 1.2,
      cursorBlink: true,
      cursorStyle: 'block',
      scrollback: 10000,
      allowProposedApi: true,
    });

    // Load addons
    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);

    const searchAddon = new SearchAddon();
    term.loadAddon(searchAddon);

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

    // Listen for PTY output.
    // Guard term.write: @xterm/addon-webgl@0.19.0 can throw on long-running
    // buffers, leaving the renderer in a corrupted state that blanks the pane
    // to only xterm's fallback glyph. On failure, dispose the WebGL addon
    // once and retry; subsequent writes use the default DOM renderer.
    unlistenOutput = await listen<{ id: string; data: number[] }>('pty-output', (event) => {
      if (event.payload.id === agentId && term) {
        const decoder = new TextDecoder();
        const text = decoder.decode(new Uint8Array(event.payload.data));
        try {
          term.write(text);
        } catch (e) {
          if (webglAddon) {
            console.error('xterm write failed, disposing WebGL addon and falling back to DOM renderer:', e);
            try { webglAddon.dispose(); } catch { /* ignore */ }
            webglAddon = null;
            try { term.write(text); } catch (e2) {
              console.error('xterm write failed after WebGL fallback:', e2);
            }
          } else {
            console.error('xterm write failed:', e);
          }
        }
      }
    });

    // Listen for status changes
    unlistenStatus = await listen<{ id: string; status: string }>('pty-status', (event) => {
      if (event.payload.id === agentId && onStatusChange) {
        onStatusChange(event.payload.status);
      }
    });

    // Handle resize
    // Guard: skip fit if terminal is hidden (visibility: hidden returns 0x0 dimensions,
    // corrupting xterm's scroll state). offsetParent is null when element or any ancestor
    // has display:none, but visibility:hidden elements still have offsetParent.
    // We check both offsetParent AND getBoundingClientRect to catch visibility:hidden.
    const handleResize = (forceFit = false) => {
      if (fitAddon && term) {
        // Skip resize if terminal is hidden unless force-fitting (e.g., after becoming visible)
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
    };

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
    term?.write(data);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
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
    <!-- svelte-ignore a11y_no_static_element_interactions -->
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
