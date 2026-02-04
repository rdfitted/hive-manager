<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Terminal as XTerm } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { WebglAddon } from '@xterm/addon-webgl';
  import { SearchAddon } from '@xterm/addon-search';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import '@xterm/xterm/css/xterm.css';

  interface Props {
    agentId: string;
    onStatusChange?: (status: string) => void;
  }

  let { agentId, onStatusChange }: Props = $props();

  let terminalContainer: HTMLDivElement;
  let term: XTerm | null = null;
  let fitAddon: FitAddon | null = null;
  let unlistenOutput: UnlistenFn | null = null;
  let unlistenStatus: UnlistenFn | null = null;
  let resizeObserver: ResizeObserver | null = null;

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

    // Handle terminal input
    term.onData(async (data) => {
      try {
        const encoder = new TextEncoder();
        const bytes = Array.from(encoder.encode(data));
        await invoke('write_to_pty', { id: agentId, data: bytes });
      } catch (err) {
        console.error('Failed to write to PTY:', err);
      }
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

<div class="terminal-wrapper" bind:this={terminalContainer}></div>

<style>
  .terminal-wrapper {
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
</style>
