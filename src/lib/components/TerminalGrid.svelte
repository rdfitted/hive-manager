<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onDestroy, tick } from 'svelte';
  import {
    ArrowsIn,
    ArrowsOut,
    Check,
    Circle,
    Hourglass,
    Plus,
    X,
  } from 'phosphor-svelte';

  import { layout } from '$lib/stores/layout';
  import {
    scratchTerminals,
    shellCommand,
    type ScratchShell,
    type ScratchTerminalPane,
  } from '$lib/stores/scratchTerminals';
  import { activeSession, serdeEnumVariantName, type AgentInfo } from '$lib/stores/sessions';
  import Terminal from './Terminal.svelte';

  interface Props {
    agents: AgentInfo[];
    focusedAgentId: string | null;
    onSelect: (id: string) => void;
  }

  interface AgentTerminalPane {
    kind: 'agent';
    id: string;
    title: string;
    agent: AgentInfo;
  }

  type TerminalPane = AgentTerminalPane | ScratchTerminalPane;

  interface TerminalReadyWaiter {
    resolve: () => void;
    reject: (error: Error) => void;
    timeout: ReturnType<typeof setTimeout>;
  }

  const TERMINAL_SESSION_STATES = new Set([
    'Completed',
    'Closed',
    'Closing',
    'Failed',
    'QaMaxRetriesExceeded',
  ]);
  const TERMINAL_READY_TIMEOUT_MS = 10_000;
  const readyTerminalIds = new Set<string>();
  const terminalReadyWaiters = new Map<string, TerminalReadyWaiter>();

  let { agents, focusedAgentId, onSelect }: Props = $props();

  let selectedShell = $state<ScratchShell>('powershell');
  let openingScratch = $state(false);
  let openingScratchId = $state<string | null>(null);
  let scratchError = $state<string | null>(null);
  let previousFocusedAgentId = $state<string | null>(null);

  let sessionId = $derived($activeSession?.id ?? null);
  let sessionState = $derived(serdeEnumVariantName($activeSession?.state));
  let scratchSessionAvailable = $derived(
    sessionId !== null && !TERMINAL_SESSION_STATES.has(sessionState ?? '')
  );
  let scratchPanes = $derived(sessionId ? ($scratchTerminals.panesBySession[sessionId] ?? []) : []);
  let focusedScratchId = $derived(sessionId ? ($scratchTerminals.focusedBySession[sessionId] ?? null) : null);
  let panes = $derived.by<TerminalPane[]>(() => [
    ...agents.map((agent): AgentTerminalPane => ({
      kind: 'agent',
      id: agent.id,
      title: getRoleLabel(agent),
      agent,
    })),
    ...scratchPanes,
  ]);
  let maximizedTerminalId = $derived($layout.maximizedTerminalId);

  let cols = $derived(
    panes.length <= 1 ? 1 :
    panes.length <= 2 ? 2 :
    panes.length <= 4 ? 2 :
    panes.length <= 6 ? 3 :
    panes.length <= 9 ? 3 :
    4
  );

  let rows = $derived(
    panes.length <= 2 ? 1 :
    panes.length <= 4 ? 2 :
    panes.length <= 6 ? 2 :
    panes.length <= 9 ? 3 :
    Math.ceil(panes.length / 4)
  );

  $effect(() => {
    const maximizedId = maximizedTerminalId;
    if (maximizedId && !panes.some((pane) => pane.id === maximizedId)) {
      layout.setMaximizedTerminalId(null);
    }
  });

  $effect(() => {
    const currentSessionId = sessionId;
    const currentSessionState = sessionState;
    if (!currentSessionId || !TERMINAL_SESSION_STATES.has(currentSessionState ?? '')) return;

    const ownedPanes = $scratchTerminals.panesBySession[currentSessionId] ?? [];
    if (ownedPanes.length > 0) {
      for (const pane of ownedPanes) {
        forgetTerminalReady(pane.id, new Error(`Session ${currentSessionId} is no longer running`));
        void invoke('kill_pty', { id: pane.id }).catch((error) => {
          scratchError = error instanceof Error ? error.message : String(error);
        });
      }
      scratchTerminals.clearSession(currentSessionId);
    }
    if (maximizedTerminalId?.startsWith(`scratch:${currentSessionId}:`)) {
      layout.setMaximizedTerminalId(null);
    }
  });

  $effect(() => {
    const nextFocusedAgentId = focusedAgentId;
    if (nextFocusedAgentId !== previousFocusedAgentId) {
      previousFocusedAgentId = nextFocusedAgentId;
      if (sessionId) scratchTerminals.focus(sessionId, null);
    }
  });

  function getRoleLabel(agent: AgentInfo) {
    if (agent.config?.label) return agent.config.label;
    if (agent.config?.name && agent.config?.description) return `${agent.config.name} — ${agent.config.description}`;
    if (agent.config?.name) return agent.config.name;
    if (serdeEnumVariantName(agent.role) === 'Queen') return 'Queen';
    if (typeof agent.role === 'object' && agent.role !== null) {
      if ('Judge' in agent.role) return 'Judge';
      if ('Planner' in agent.role) return `Planner ${agent.role.Planner.index}`;
      if ('Worker' in agent.role) return `Worker ${agent.role.Worker.index}`;
      if ('QaWorker' in agent.role) return `QA Worker ${agent.role.QaWorker.index}`;
    }
    if (serdeEnumVariantName(agent.role) === 'Evaluator') return 'Evaluator';
    return 'Agent';
  }

  function focusPane(pane: TerminalPane) {
    if (pane.kind === 'scratch') {
      scratchTerminals.focus(pane.sessionId, pane.id);
      return;
    }

    if (sessionId) scratchTerminals.focus(sessionId, null);
    onSelect(pane.id);
  }

  function isPaneFocused(pane: TerminalPane) {
    return pane.kind === 'scratch'
      ? pane.id === focusedScratchId
      : focusedScratchId === null && pane.id === focusedAgentId;
  }

  function toggleMaximized(event: MouseEvent, pane: TerminalPane) {
    event.stopPropagation();
    focusPane(pane);
    layout.toggleMaximizedTerminal(pane.id);
  }

  function handleWindowKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && maximizedTerminalId) {
      layout.setMaximizedTerminalId(null);
    }
  }

  function markTerminalReady(id: string) {
    readyTerminalIds.add(id);
    const waiter = terminalReadyWaiters.get(id);
    if (!waiter) return;

    clearTimeout(waiter.timeout);
    terminalReadyWaiters.delete(id);
    waiter.resolve();
  }

  function waitForTerminalReady(id: string): Promise<void> {
    if (readyTerminalIds.has(id)) return Promise.resolve();

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        terminalReadyWaiters.delete(id);
        reject(new Error('Terminal listeners did not become ready in time'));
      }, TERMINAL_READY_TIMEOUT_MS);
      terminalReadyWaiters.set(id, { resolve, reject, timeout });
    });
  }

  function forgetTerminalReady(id: string, error?: Error) {
    readyTerminalIds.delete(id);
    const waiter = terminalReadyWaiters.get(id);
    if (!waiter) return;

    clearTimeout(waiter.timeout);
    terminalReadyWaiters.delete(id);
    waiter.reject(error ?? new Error(`Terminal ${id} was removed before it became ready`));
  }

  function removeScratchPane(pane: ScratchTerminalPane) {
    forgetTerminalReady(pane.id);
    scratchTerminals.remove(pane.sessionId, pane.id);
    if (maximizedTerminalId === pane.id) layout.setMaximizedTerminalId(null);
  }

  function handlePaneStatus(pane: TerminalPane, status: string) {
    if (pane.kind === 'scratch' && status === 'Completed') {
      void finalizeCompletedScratchPane(pane);
    }
  }

  async function finalizeCompletedScratchPane(pane: ScratchTerminalPane) {
    try {
      // The PTY reader reports Completed after a shell exits naturally. Route that
      // through kill as well so the manager drops its dead handle and ownership record.
      await invoke('kill_pty', { id: pane.id });
    } catch (error) {
      scratchError = error instanceof Error ? error.message : String(error);
    } finally {
      removeScratchPane(pane);
    }
  }

  async function openScratchTerminal() {
    const session = $activeSession;
    if (!session || !scratchSessionAvailable || openingScratch) return;

    const cwd = session.worktree_path?.trim() || session.project_path;
    const pane = scratchTerminals.add(session.id, cwd, selectedShell);
    const { command, args } = shellCommand(selectedShell);

    openingScratch = true;
    openingScratchId = pane.id;
    scratchError = null;

    try {
      // Mount Terminal and its event listeners before the shell starts emitting output.
      await tick();
      await waitForTerminalReady(pane.id);
      await invoke<string>('create_pty', {
        id: pane.id,
        command,
        args,
        cwd,
        cols: 120,
        rows: 30,
        role: 'scratch_shell',
        shell: selectedShell,
        sessionId: session.id,
      });
    } catch (error) {
      removeScratchPane(pane);
      scratchError = error instanceof Error ? error.message : String(error);
    } finally {
      openingScratch = false;
      if (openingScratchId === pane.id) openingScratchId = null;
    }
  }

  async function closeScratchTerminal(event: MouseEvent, pane: ScratchTerminalPane) {
    event.stopPropagation();
    if (openingScratchId === pane.id) return;
    scratchError = null;

    try {
      await invoke('kill_pty', { id: pane.id });
      removeScratchPane(pane);
    } catch (error) {
      scratchError = error instanceof Error ? error.message : String(error);
    }
  }

  onDestroy(() => {
    for (const [id, waiter] of terminalReadyWaiters) {
      clearTimeout(waiter.timeout);
      waiter.reject(new Error(`Terminal ${id} was destroyed before it became ready`));
    }
    terminalReadyWaiters.clear();
    readyTerminalIds.clear();
  });
</script>

<svelte:window onkeydowncapture={handleWindowKeydown} />

<div class="terminal-workspace">
  <div class="terminal-toolbar">
    <span class="toolbar-label">Terminals</span>
    <div class="scratch-controls">
      <label class="shell-picker">
        <span class="sr-only">Scratch terminal shell</span>
        <select bind:value={selectedShell} disabled={!scratchSessionAvailable || openingScratch}>
          <option value="powershell">PowerShell</option>
          <option value="cmd">Command Prompt</option>
        </select>
      </label>
      <button
        type="button"
        class="open-terminal-button"
        disabled={!scratchSessionAvailable || openingScratch}
        onclick={openScratchTerminal}
      >
        <Plus size={13} weight="bold" />
        {openingScratch ? 'Opening…' : 'Open Terminal'}
      </button>
    </div>
  </div>

  {#if scratchError}
    <div class="scratch-error" role="alert">Could not update scratch terminal: {scratchError}</div>
  {/if}

  <div
    class="terminal-grid"
    style="--cols: {cols}; --rows: {rows}"
    class:scrollable={panes.length > 9}
    class:has-maximized={maximizedTerminalId !== null}
    class:empty={panes.length === 0}
  >
    {#each panes as pane (pane.id)}
      {@const focused = isPaneFocused(pane)}
      <!-- Container click is a convenience; the dedicated header button carries
           accessible focus semantics without wrapping the terminal or other controls. -->
      <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
      <div
        class="terminal-item"
        class:focused
        class:maximized={pane.id === maximizedTerminalId}
        class:hidden-by-maximize={maximizedTerminalId !== null && pane.id !== maximizedTerminalId}
        onclick={() => focusPane(pane)}
      >
        <div
          class="terminal-header"
          style:border-top-color={$activeSession?.color || 'transparent'}
          style:border-top-width={$activeSession?.color ? '3px' : '0'}
        >
          <button
            type="button"
            class="terminal-focus-button"
            aria-label={`Focus ${pane.title}`}
            onclick={(event) => {
              event.stopPropagation();
              focusPane(pane);
            }}
          >
            <span class="role-label" title={pane.title}>{pane.title}</span>
          </button>
          <div class="terminal-meta">
            {#if pane.kind === 'agent'}
              {@const status = serdeEnumVariantName(pane.agent.status)}
              <span class="cli-badge">{pane.agent.config?.cli || 'unknown'}</span>
              <span
                class="status-indicator"
                class:waiting={typeof pane.agent.status === 'object' && 'WaitingForInput' in pane.agent.status}
                class:running={status === 'Running'}
                class:completed={status === 'Completed'}
              >
                {#if status === 'Running'}
                  █
                {:else if typeof pane.agent.status === 'object' && 'WaitingForInput' in pane.agent.status}
                  <Hourglass size={10} weight="light" />
                {:else if status === 'Completed'}
                  <Check size={10} weight="light" />
                {:else}
                  <Circle size={10} weight="light" />
                {/if}
              </span>
            {:else}
              <span class="cli-badge">{pane.shell}</span>
              <span class="cwd-label" title={`${pane.cwd} · opened ${pane.createdAt}`}>{pane.cwd}</span>
              <button
                type="button"
                class="pane-action close-button"
                aria-label={`Close ${pane.title}`}
                title="Close scratch terminal"
                disabled={openingScratchId === pane.id}
                onclick={(event) => closeScratchTerminal(event, pane)}
              >
                <X size={13} weight="light" />
              </button>
            {/if}
            <button
              type="button"
              class="pane-action maximize-button"
              aria-label={pane.id === maximizedTerminalId ? `Restore ${pane.title}` : `Maximize ${pane.title}`}
              title={pane.id === maximizedTerminalId ? 'Restore terminal (Esc)' : 'Maximize terminal'}
              onclick={(event) => toggleMaximized(event, pane)}
            >
              {#if pane.id === maximizedTerminalId}
                <ArrowsIn size={13} weight="light" />
              {:else}
                <ArrowsOut size={13} weight="light" />
              {/if}
            </button>
          </div>
        </div>
        <div class="terminal-container">
          <Terminal
            agentId={pane.id}
            isAgent={pane.kind === 'agent'}
            isFocused={focused}
            isVisible={maximizedTerminalId === null || pane.id === maximizedTerminalId}
            layoutRevision={maximizedTerminalId}
            onReady={pane.kind === 'scratch' ? () => markTerminalReady(pane.id) : undefined}
            onStatusChange={pane.kind === 'scratch' ? (status) => handlePaneStatus(pane, status) : undefined}
          />
        </div>
      </div>
    {/each}

    {#if panes.length === 0}
      <div class="empty-state">
        <span>No terminals are running.</span>
        <span>Open a scratch terminal to work in this session.</span>
      </div>
    {/if}
  </div>
</div>

<style>
  .terminal-workspace {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    min-height: 0;
  }

  .terminal-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 4px 8px;
  }

  .toolbar-label {
    color: var(--text-secondary);
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .scratch-controls {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .shell-picker select,
  .open-terminal-button {
    height: 28px;
    color: var(--text-secondary);
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    font-size: 10px;
  }

  .shell-picker select {
    padding: 0 24px 0 8px;
  }

  .open-terminal-button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 0 10px;
    cursor: pointer;
  }

  .open-terminal-button:hover:not(:disabled),
  .open-terminal-button:focus-visible {
    color: var(--accent-cyan);
    background: var(--bg-elevated);
    outline: none;
  }

  .open-terminal-button:disabled,
  .shell-picker select:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  .scratch-error {
    margin: 0 4px 6px;
    padding: 5px 8px;
    color: var(--status-error);
    background: color-mix(in srgb, var(--status-error) 8%, var(--bg-surface));
    border: 1px solid color-mix(in srgb, var(--status-error) 30%, var(--border-structural));
    border-radius: var(--radius-sm);
    font-size: 10px;
  }

  .terminal-grid {
    display: grid;
    flex: 1;
    grid-template-columns: repeat(var(--cols), 1fr);
    grid-template-rows: repeat(var(--rows), 1fr);
    gap: 12px;
    width: 100%;
    min-height: 0;
    padding: 4px;
    overflow: hidden;
  }

  .terminal-grid.scrollable {
    grid-template-rows: repeat(var(--rows), 300px);
    overflow-y: auto;
  }

  .terminal-grid.has-maximized {
    position: relative;
    overflow: hidden;
  }

  .terminal-grid.empty {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .terminal-item {
    display: flex;
    flex-direction: column;
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
    transition: border-color 0.2s, box-shadow 0.2s;
    min-height: 0;
  }

  .terminal-item.focused {
    border-color: var(--accent-cyan);
    box-shadow: 0 0 0 1px var(--accent-cyan);
  }

  .terminal-item.maximized {
    position: absolute;
    inset: 4px;
    z-index: 2;
  }

  .terminal-item.hidden-by-maximize {
    position: absolute;
    inset: 4px;
    visibility: hidden;
    pointer-events: none;
  }

  .terminal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 6px 10px;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border-structural);
    user-select: none;
  }

  .role-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-primary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .terminal-meta {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .terminal-focus-button {
    display: flex;
    flex: 1;
    min-width: 0;
    padding: 0;
    color: inherit;
    text-align: left;
    background: transparent;
    border: 0;
    cursor: pointer;
  }

  .terminal-focus-button:focus-visible {
    outline: 1px solid var(--accent-cyan);
    outline-offset: 2px;
  }

  .pane-action {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    color: var(--text-secondary);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .pane-action:hover,
  .pane-action:focus-visible {
    color: var(--accent-cyan);
    background: var(--bg-elevated);
    border-color: var(--border-structural);
    outline: none;
  }

  .pane-action:disabled {
    cursor: wait;
    opacity: 0.45;
  }

  .close-button:hover,
  .close-button:focus-visible {
    color: var(--status-error);
  }

  .cli-badge {
    font-size: 9px;
    padding: 1px 4px;
    background: var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    text-transform: lowercase;
  }

  .cwd-label {
    max-width: 180px;
    overflow: hidden;
    color: var(--text-secondary);
    font-family: var(--font-mono);
    font-size: 9px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status-indicator {
    font-size: 10px;
  }

  .status-indicator.running {
    color: var(--accent-cyan);
  }

  .status-indicator.waiting {
    color: var(--status-warning);
    animation: pulse 2s infinite;
  }

  .status-indicator.completed {
    color: var(--status-success);
  }

  @keyframes pulse {
    0% { opacity: 1; }
    50% { opacity: 0.5; }
    100% { opacity: 1; }
  }

  .terminal-container {
    flex: 1;
    min-height: 0;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    color: var(--text-secondary);
    font-size: 11px;
    text-align: center;
  }

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
</style>
