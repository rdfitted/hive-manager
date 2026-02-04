<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Terminal as XTerm } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { WebglAddon } from '@xterm/addon-webgl';
  import { SearchAddon } from '@xterm/addon-search';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
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

  onMount(async () => {
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

<div class="terminal-wrapper" bind:this={terminalContainer}>
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
</style>
