<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Terminal as XTerm } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { WebglAddon } from '@xterm/addon-webgl';
  import { SearchAddon } from '@xterm/addon-search';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
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
  let unlistenOutput: UnlistenFn | null = null;
  let unlistenStatus: UnlistenFn | null = null;
  let resizeObserver: ResizeObserver | null = null;

  // Context menu state
  let showContextMenu = $state(false);
  let contextMenuX = $state(0);
  let contextMenuY = $state(0);
  let hasSelection = $state(false);

  // Track agent status from store
  let agent = $derived($activeAgents.find(a => a.id === agentId));
  let isWaiting = $derived(agent?.status && typeof agent.status === 'object' && 'WaitingForInput' in agent.status);

  $effect(() => {
    if (isFocused && term) {
      term.focus();
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

  async function sendToPty(data: string) {
    try {
      const encoder = new TextEncoder();
      const bytes = Array.from(encoder.encode(data));
      await invoke('write_to_pty', { id: agentId, data: bytes });
    } catch (err) {
      console.error('[Terminal] Failed to write to PTY:', err);
    }
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
      sendToPty(text);
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

  // Handle paste events from external tools (like Wispr Flow)
  function handlePasteEvent(event: ClipboardEvent) {
    const text = event.clipboardData?.getData('text');
    if (text && term) {
      event.preventDefault();
      sendToPty(text);
    }
  }

  // Global paste handler for when terminal has focus but paste targets document
  function handleGlobalPaste(event: ClipboardEvent) {
    // Only handle if this terminal is focused
    if (!isFocused || !term) return;

    // Check if target is already our terminal (avoid double handling)
    if (terminalContainer?.contains(event.target as Node)) return;

    const text = event.clipboardData?.getData('text');
    if (text) {
      event.preventDefault();
      sendToPty(text);
    }
  }

  onMount(async () => {
    // Add global click listener
    document.addEventListener('click', handleGlobalClick);
    // Add global paste listener for tools like Wispr Flow
    document.addEventListener('paste', handleGlobalPaste);
    // Create terminal instance
    term = new XTerm({
      theme: tokyoNightTheme,
      fontFamily: 'Cascadia Code, Consolas, monospace',
      fontSize: 14,
      lineHeight: 1.2,
      cursorBlink: true,
      cursorStyle: 'block',
      allowProposedApi: true,
    });

    // Load addons
    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);

    const searchAddon = new SearchAddon();
    term.loadAddon(searchAddon);

    // Open terminal in container
    term.open(terminalContainer);

    // Add paste event listener for external tools like Wispr Flow
    terminalContainer.addEventListener('paste', handlePasteEvent);

    // Try to load WebGL addon for better performance
    try {
      const webglAddon = new WebglAddon();
      term.loadAddon(webglAddon);
    } catch (e) {
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
      if (event.key === 'Enter' && event.shiftKey) {
        if (term) {
          term.write('\r\n');
          sendToPty('\n');
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
      if (event.ctrlKey && (event.key === 'V' || event.key === 'v')) {
        (async () => {
          let text: string | null = null;
          try {
            text = await readText();
          } catch {
            // Tauri API failed, try browser
          }
          if (!text) {
            try {
              text = await navigator.clipboard.readText();
            } catch {
              // Browser API also failed
            }
          }
          if (text && term) {
            sendToPty(text);
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

    // Listen for PTY output
    unlistenOutput = await listen<{ id: string; data: number[] }>('pty-output', (event) => {
      if (event.payload.id === agentId && term) {
        const decoder = new TextDecoder();
        const text = decoder.decode(new Uint8Array(event.payload.data));
        term.write(text);
      }
    });

    // Listen for status changes
    unlistenStatus = await listen<{ id: string; status: string }>('pty-status', (event) => {
      if (event.payload.id === agentId && onStatusChange) {
        onStatusChange(event.payload.status);
      }
    });

    // Handle resize
    resizeObserver = new ResizeObserver(() => {
      if (fitAddon && term) {
        fitAddon.fit();
        const dims = fitAddon.proposeDimensions();
        if (dims) {
          invoke('resize_pty', { id: agentId, cols: dims.cols, rows: dims.rows }).catch(console.error);
        }
      }
    });
    resizeObserver.observe(terminalContainer);

    // Report initial size
    const dims = fitAddon.proposeDimensions();
    if (dims) {
      await invoke('resize_pty', { id: agentId, cols: dims.cols, rows: dims.rows }).catch(console.error);
    }
  });

  onDestroy(() => {
    document.removeEventListener('click', handleGlobalClick);
    document.removeEventListener('paste', handleGlobalPaste);
    terminalContainer?.removeEventListener('paste', handlePasteEvent);
    if (unlistenOutput) unlistenOutput();
    if (unlistenStatus) unlistenStatus();
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
>
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
        <span class="context-icon">📋</span> Copy
        <span class="context-shortcut">Ctrl+C</span>
      </button>
      <button class="context-item" onclick={handlePaste}>
        <span class="context-icon">📄</span> Paste
        <span class="context-shortcut">Ctrl+V</span>
      </button>
      <div class="context-divider"></div>
      <button class="context-item" onclick={handleSelectAll}>
        <span class="context-icon">☑</span> Select All
        <span class="context-shortcut">Ctrl+A</span>
      </button>
      <button class="context-item" onclick={handleClearSelection} disabled={!hasSelection}>
        <span class="context-icon">✕</span> Clear Selection
      </button>
    </div>
  {/if}
</div>

<style>
  .terminal-wrapper {
    position: relative;
    width: 100%;
    height: 100%;
    background: #1a1b26;
    border-radius: 4px;
    overflow: hidden;
  }

  .terminal-wrapper :global(.xterm) {
    padding: 8px;
    height: 100%;
  }

  .terminal-wrapper :global(.xterm-viewport) {
    background: #1a1b26 !important;
  }

  .quick-response-overlay {
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    padding: 8px 12px;
    background: rgba(26, 27, 38, 0.85);
    backdrop-filter: blur(4px);
    border-top: 1px solid var(--color-warning);
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
    color: var(--color-warning);
    font-weight: 600;
    text-transform: uppercase;
    margin-right: 4px;
  }

  .action-btn {
    padding: 4px 12px;
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    color: var(--color-text);
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .action-btn:hover {
    background: var(--color-surface-hover);
    border-color: var(--color-accent);
  }

  .action-btn.approve {
    background: rgba(158, 206, 106, 0.15);
    border-color: var(--color-success);
    color: var(--color-success);
  }

  .action-btn.approve:hover {
    background: rgba(158, 206, 106, 0.25);
  }

  .action-btn.reject {
    background: rgba(247, 118, 142, 0.15);
    border-color: var(--color-error);
    color: var(--color-error);
  }

  .action-btn.reject:hover {
    background: rgba(247, 118, 142, 0.25);
  }

  /* Context Menu */
  .context-menu {
    position: fixed;
    background: var(--color-surface, #24283b);
    border: 1px solid var(--color-border, #414868);
    border-radius: 6px;
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
    border-radius: 4px;
    color: var(--color-text, #c0caf5);
    font-size: 13px;
    cursor: pointer;
    text-align: left;
  }

  .context-item:hover:not(:disabled) {
    background: var(--color-surface-hover, #2f3549);
  }

  .context-item:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .context-icon {
    font-size: 14px;
    width: 20px;
    text-align: center;
  }

  .context-shortcut {
    margin-left: auto;
    font-size: 11px;
    color: var(--color-text-muted, #565f89);
  }

  .context-divider {
    height: 1px;
    background: var(--color-border, #414868);
    margin: 4px 8px;
  }
</style>
