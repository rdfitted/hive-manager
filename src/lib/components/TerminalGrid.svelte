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
  import { agentEvents } from '$lib/stores/agentEvents';
  import {
    scratchTerminals,
    shellCommand,
    type ScratchShell,
    type ScratchTerminalPane,
  } from '$lib/stores/scratchTerminals';
  import { sessions, serdeEnumVariantName, resumeBlockedReason, type AgentInfo, type Session } from '$lib/stores/sessions';
  import { TERMINAL_SESSION_STATES } from '$lib/stores/terminalPool';
  import AgentActivityLine from './agent/AgentActivityLine.svelte';
  import AttentionCard from './attention/AttentionCard.svelte';
  import { createFlushState, flushKey } from './attention/flushState';
  import { attentionVisible } from './attention/visibility';
  import { startupExitCode, workStateFor, agentActivity, now, staleRunningAgents } from '$lib/stores/agentActivity';
  import { heartbeatStore } from '$lib/stores/conversations';
  import Terminal from './Terminal.svelte';
  import ChatPane from './chat/ChatPane.svelte';
  import PaneToggle from './chat/PaneToggle.svelte';
  import { protocolAttention } from './chat/transcript';
  import CommandBar from './opswall/CommandBar.svelte';
  import OpsWall from './opswall/OpsWall.svelte';
  import { columnPins } from './opswall/pinning';

  interface Props {
    session: Session;
    /** False while this grid is pooled behind another session or a non-terminal tab (#286). */
    visible?: boolean;
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

  const TERMINAL_READY_TIMEOUT_MS = 10_000;
  const isWindows = typeof navigator !== 'undefined' && navigator.platform.toLowerCase().startsWith('win');
  const readyTerminalIds = new Set<string>();
  const terminalReadyWaiters = new Map<string, TerminalReadyWaiter>();
  const flushState = createFlushState();

  let { session, visible = true, focusedAgentId, onSelect }: Props = $props();

  let agents = $derived(session.agents);

  let selectedShell = $state<ScratchShell>(isWindows ? 'powershell' : 'login');
  let openingScratch = $state(false);
  let openingScratchId = $state<string | null>(null);
  let scratchError = $state<string | null>(null);
  let previousFocusedAgentId = $state<string | null>(null);
  let paneModes = $state<Record<string, 'chat' | 'terminal' | 'raw'>>({});
  let terminalFocusId = $state<string | null>(null);
  let resumingAgents = $state<Record<string, boolean>>({});
  let resumeErrors = $state<Record<string, string>>({});
  let commandTargetId = $state<string | null>(null);
  let drafts = $state<Record<string, string>>({});
  let delivery = $state<Record<string, string>>({});
  let resourceFilter = $state<string | null>(null);
  let wall: HTMLDivElement;
  let pins = $state<Record<string, number>>({});
  let pinSpace = $state(0);
  const columnNodes = new Map<string, HTMLElement>();
  let pinObserver: ResizeObserver | undefined;
  let pinMeasureQueued = false;
  let destroyed = false;
  let savedGridScrollLeft = 0;

  function schedulePinMeasure() {
    if (pinMeasureQueued || destroyed) return;
    pinMeasureQueued = true;
    void tick().then(() => {
      pinMeasureQueued = false;
      if (destroyed || !wall || focusMode || localMaximizedId) return;
      const candidates = session.agents.map(agent => ({ id: agent.id,
        width: columnNodes.get(agent.id)?.getBoundingClientRect().width ?? 0,
        queen: serdeEnumVariantName(agent.role) === 'Queen', attention: attentionSeats.has(agent.id),
      }));
      const next = columnPins(wall.clientWidth, candidates);
      if (JSON.stringify(next) !== JSON.stringify(pins)) pins = next;
      pinSpace = Math.max(0, ...candidates.filter(column => next[column.id] !== undefined).map(column => next[column.id] + column.width + 12));
      void tick().then(updateColumnClips);
    });
  }
  function measureWall(node: HTMLDivElement) {
    if (typeof ResizeObserver !== 'undefined') {
      pinObserver = new ResizeObserver(schedulePinMeasure);
      pinObserver.observe(node);
      for (const column of columnNodes.values()) pinObserver.observe(column);
    }
    schedulePinMeasure();
    return { destroy() { pinObserver?.disconnect(); } };
  }
  function measureColumn(node: HTMLDivElement, id: string) {
    columnNodes.set(id, node); pinObserver?.observe(node); schedulePinMeasure();
    return { destroy() { pinObserver?.unobserve?.(node); columnNodes.delete(id); } };
  }
  function updateColumnClips() {
    if (!wall || destroyed) return;
    const start = wall.getBoundingClientRect().left + pinSpace;
    const updates = [...columnNodes].map(([id, node]) => {
      const box = node.getBoundingClientRect();
      return { node, clip: focusMode || localMaximizedId || pins[id] !== undefined ? 0 : Math.min(box.width, Math.max(0, start - box.left)) };
    });
    for (const { node, clip } of updates) node.style.setProperty('--wall-clip', `${clip}px`);
  }
  $effect(() => { void attentionSeats; void focusMode; void localMaximizedId; schedulePinMeasure(); });
  $effect(() => { if ((focusMode || localMaximizedId !== null) && wall) wall.scrollLeft = 0; });
  const staleSeats = $derived.by(() => {
    const last = Object.fromEntries(session.agents.map(agent => {
      const history = $agentEvents.sessionId === session.id ? $agentEvents.histories[agent.id]?.events ?? [] : [];
      const times = [Date.parse($heartbeatStore.agents[agent.id]?.timestamp ?? ''), Date.parse(agent.latest_input_at ?? ''), $agentActivity[agent.id]?.lastActivityAt ?? NaN, ...history.map(event => Date.parse(event.ts))].filter(Number.isFinite);
      return [agent.id, times.length ? Math.max(...times) : null];
    }));
    return staleRunningAgents(session.agents.map(agent => ({ ...agent, work_state: workStateFor(agent, $heartbeatStore.agents[agent.id]) })), last, $now, Date.parse(session.created_at));
  });
  function hasAgentAttention(agent: AgentInfo) {
    const scoped = $agentEvents.sessionId === session.id;
    const feed = scoped ? $agentEvents.feeds[agent.id] : undefined;
    const chatAvailable = !!feed && feed.capabilities.event_source !== 'none';
    const history = scoped ? $agentEvents.histories[agent.id]?.events ?? [] : [];
    return attentionVisible(agent, $heartbeatStore.staleAgents, workStateFor(agent, $heartbeatStore.agents[agent.id]), {
      chatAvailable, eventAttention: protocolAttention(history),
      feedError: chatAvailable ? $agentEvents.error ?? $agentEvents.histories[agent.id]?.error : null,
    }) || !!$flushState[flushKey(session.id, agent.id)];
  }
  const attentionSeats = $derived(new Set(session.agents.filter(hasAgentAttention).map(agent => agent.id)));

  function revealAgent(id: string) {
    const pane = panes.find(candidate => candidate.id === id);
    if (pane) focusPane(pane);
    wall?.querySelector<HTMLElement>(`[data-agent-id=${JSON.stringify(id)}]`)?.scrollIntoView?.({ block: 'nearest', inline: 'nearest', behavior: 'smooth' });
  }
  function filterResources(filter: string | null) {
    resourceFilter = filter;
    const id = filter === 'attention' ? [...attentionSeats][0] : filter === 'stale' ? [...staleSeats][0]
      : filter === 'waiting' ? session.agents.find(agent => serdeEnumVariantName(agent.status) === 'WaitingForInput')?.id : undefined;
    if (id) revealAgent(id);
  }

  function deliveryLabel(agent: AgentInfo): string | null {
    if (typeof agent.status === 'object' && 'WaitingForInput' in agent.status) {
      if (agent.status.WaitingForInput.startsWith('injected_delivery_failed:')) return 'Delivery failed';
      if (agent.status.WaitingForInput.startsWith('awaiting_injected_turn')) return 'Awaiting turn';
    }
    return delivery[`${session.id}:${agent.id}`] ?? null;
  }

  async function resumeAgent(event: MouseEvent, agentId: string) {
    event.stopPropagation();
    if (resumingAgents[agentId]) return;
    resumingAgents[agentId] = true;
    delete resumeErrors[agentId];
    try {
      await sessions.resumeAgent(session.id, agentId);
    } catch (error) {
      resumeErrors[agentId] = String(error);
    } finally {
      resumingAgents[agentId] = false;
    }
  }

  let sessionId = $derived(session.id);
  let sessionState = $derived(serdeEnumVariantName(session.state));
  let scratchSessionAvailable = $derived(!TERMINAL_SESSION_STATES.has(sessionState ?? ''));
  let scratchPanes = $derived(sessionId ? ($scratchTerminals.panesBySession[sessionId] ?? []) : []);
  let focusedScratchId = $derived(sessionId ? ($scratchTerminals.focusedBySession[sessionId] ?? null) : null);
  let panes = $derived.by<TerminalPane[]>(() => [
    ...[...agents].sort((a, b) => focusMode ? 0 : Number(serdeEnumVariantName(b.role) === 'Queen') - Number(serdeEnumVariantName(a.role) === 'Queen') || Number(pins[b.id] !== undefined) - Number(pins[a.id] !== undefined)).map((agent): AgentTerminalPane => ({
      kind: 'agent',
      id: agent.id,
      title: getRoleLabel(agent),
      agent,
    })),
    ...scratchPanes,
  ]);
  let maximizedTerminalId = $derived($layout.maximizedTerminalId);
  let focusMode = $derived($layout.sessionViewMode === 'focus');
  let focusedPaneId = $derived(
    scratchPanes.some((pane) => pane.id === focusedScratchId) ? focusedScratchId
      : agents.some((agent) => agent.id === focusedAgentId) ? focusedAgentId
      : panes[0]?.id ?? null
  );
  // The maximize id is global; only honour it when it names one of this grid's panes,
  // so a pooled grid never collapses because another session maximized something.
  let gridMaximizedId = $derived(
    maximizedTerminalId !== null && panes.some((pane) => pane.id === maximizedTerminalId)
      ? maximizedTerminalId
      : null
  );
  // Focus reuses the maximize geometry, never the global maximize state or a filtered
  // pane list. Hidden panes retain their Terminal instances and live scrollback.
  let localMaximizedId = $derived(focusMode ? focusedPaneId : gridMaximizedId);

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
    if (!visible) return;
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
      if (agents.some(agent => agent.id === nextFocusedAgentId)) commandTargetId = nextFocusedAgentId;
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
    commandTargetId = pane.id;
    onSelect(pane.id);
  }

  function isPaneFocused(pane: TerminalPane) {
    if (focusMode) return pane.id === localMaximizedId;
    return pane.kind === 'scratch'
      ? pane.id === focusedScratchId
      : focusedScratchId === null && pane.id === focusedAgentId;
  }

  function toggleMaximized(event: MouseEvent, pane: TerminalPane) {
    event.stopPropagation();
    const restoring = gridMaximizedId === pane.id;
    if (!restoring && wall) { savedGridScrollLeft = wall.scrollLeft; wall.scrollLeft = 0; }
    focusPane(pane);
    layout.toggleMaximizedTerminal(pane.id);
    if (restoring) void tick().then(() => { if (wall) { wall.scrollLeft = savedGridScrollLeft; updateColumnClips(); } });
  }

  function handleWindowKeydown(event: KeyboardEvent) {
    if (
      focusMode ||
      !visible ||
      event.key !== 'Escape' ||
      !localMaximizedId ||
      event.defaultPrevented ||
      document.querySelector('[aria-modal="true"]') !== null
    ) return;

    event.preventDefault();
    layout.setMaximizedTerminalId(null);
    void tick().then(() => { if (wall) { wall.scrollLeft = savedGridScrollLeft; updateColumnClips(); } });
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
    if (!scratchSessionAvailable || openingScratch) return;

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
    destroyed = true;
    pinObserver?.disconnect();
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
    {#if focusMode && panes.length > 0}
      <label class="pane-picker">
        <span class="sr-only">Focused terminal</span>
        <select
          class="lattice-input"
          value={localMaximizedId ?? ''}
          onchange={(event) => {
            const pane = panes.find((candidate) => candidate.id === event.currentTarget.value);
            if (pane) focusPane(pane);
          }}
        >
          {#each panes as pane (pane.id)}
            <option value={pane.id}>{pane.title}{pane.kind === 'scratch' ? ' (scratch)' : ''}</option>
          {/each}
        </select>
      </label>
    {:else}
      <span class="toolbar-label">Terminals</span>
    {/if}
    <div class="scratch-controls">
      <label class="shell-picker">
        <span class="sr-only">Scratch terminal shell</span>
        <select class="lattice-input" bind:value={selectedShell} disabled={!scratchSessionAvailable || openingScratch}>
          {#if isWindows}
            <option value="powershell">PowerShell</option>
            <option value="cmd">Command Prompt</option>
          {:else}
            <option value="login">Login Shell</option>
          {/if}
        </select>
      </label>
      <button
        type="button"
        class="open-terminal-button lattice-btn lattice-btn--secondary lattice-btn--compact"
        disabled={!scratchSessionAvailable || openingScratch}
        onclick={openScratchTerminal}
      >
        <Plus size={13} weight="bold" />
        {openingScratch ? 'Opening…' : 'Open Terminal'}
      </button>
    </div>
  </div>

  {#if scratchError}
    <div class="scratch-error status-badge status-error" role="alert">Could not update scratch terminal: {scratchError}</div>
  {/if}

  {#if !focusMode && visible}
    <OpsWall {session} attention={attentionSeats} stale={staleSeats} filter={resourceFilter} onfilter={filterResources} onselect={revealAgent} />
  {/if}

  <div
    bind:this={wall}
    use:measureWall
    onscroll={updateColumnClips}
    class="terminal-grid lattice-scroll-content"
    style="--cols: {cols}; --rows: {rows}; --pin-space: {pinSpace}px; --wall-tracks: {panes.map((pane) => pane.kind === 'agent' && serdeEnumVariantName(pane.agent.role) === 'Queen' ? 'minmax(420px, 1.5fr)' : 'minmax(280px, 1fr)').join(' ')}"
    class:ops-wall={!focusMode}
    class:scrollable={panes.length > 9}
    class:has-maximized={localMaximizedId !== null}
    class:empty={panes.length === 0}
  >
    {#each panes as pane (pane.id)}
      {@const focused = isPaneFocused(pane)}
      {@const feed = pane.kind === 'agent' && $agentEvents.sessionId === session.id ? $agentEvents.feeds[pane.id] : undefined}
      {@const chatEligible = !!feed && feed.capabilities.event_source !== 'none'}
      {@const hasTerminal = feed?.capabilities.has_terminal !== false}
      {@const history = $agentEvents.sessionId === session.id ? $agentEvents.histories[pane.id]?.events ?? [] : []}
      {@const defaultMode = hasTerminal && feed?.capabilities.event_source === 'transcript' && history.length === 0 ? 'terminal' : 'chat'}
      {@const mode = chatEligible ? (paneModes[pane.id] ?? defaultMode) : 'terminal'}
      {@const chatShowing = chatEligible && (mode !== 'terminal' || !hasTerminal)}
      {@const eventAttention = protocolAttention(history)}
      {@const feedError = chatEligible ? $agentEvents.error ?? $agentEvents.histories[pane.id]?.error : null}
      {@const showAttention = pane.kind === 'agent' && attentionSeats.has(pane.id)}
      {@const gridAttention = !focusMode && visible && showAttention && pane.kind === 'agent'
        && (chatEligible || startupExitCode(pane.agent) !== null || !!$flushState[flushKey(session.id, pane.id)])}
      <!-- Container click is a convenience; the dedicated header button carries
           accessible focus semantics without wrapping the terminal or other controls. -->
      <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
      <div
        use:measureColumn={pane.id}
        class={`terminal-item lattice-panel${focused ? ' lattice-panel--active' : ''}`}
        class:maximized={pane.id === localMaximizedId}
        class:hidden-by-maximize={localMaximizedId !== null && pane.id !== localMaximizedId}
        class:queen-column={!focusMode && pane.kind === 'agent' && serdeEnumVariantName(pane.agent.role) === 'Queen'}
        class:attention-column={!focusMode && showAttention}
        class:pinned-column={!focusMode && localMaximizedId === null && pins[pane.id] !== undefined}
        style:--pin-left={`${pins[pane.id] ?? 0}px`}
        data-pinned={!focusMode && localMaximizedId === null && pins[pane.id] !== undefined ? 'true' : undefined}
        class:quiet-column={!focusMode && pane.kind === 'agent' && !showAttention && (serdeEnumVariantName(pane.agent.status) === 'Idle' || pane.agent.status === 'Completed' || $heartbeatStore.agents[pane.id]?.status === 'idle' || ['finished', 'retired', 'idle'].includes(workStateFor(pane.agent, $heartbeatStore.agents[pane.id]) ?? ''))}
        class:resource-match={!focusMode && resourceFilter !== null && pane.kind === 'agent' && (resourceFilter === 'attention' ? attentionSeats.has(pane.id) : resourceFilter === 'stale' ? staleSeats.has(pane.id) : serdeEnumVariantName(pane.agent.status) === 'WaitingForInput')}
        data-agent-id={pane.kind === 'agent' ? pane.id : undefined}
        onclick={() => focusPane(pane)}
      >
        <div
          class="terminal-header"
          style:--session-color={session.color || 'transparent'}
        >
          <button
            type="button"
            class="terminal-pane-focus lattice-btn lattice-btn--ghost"
            aria-label={`Focus ${pane.title}`}
            onclick={(event) => {
              event.stopPropagation();
              focusPane(pane);
            }}
          >
            <span class="role-label" title={pane.title}>{pane.title}</span>
          </button>
          {#if !focusMode && visible && pane.kind === 'agent'}
            <span class="pane-activity"><AgentActivityLine agentId={pane.id} compact /></span>
            {#if showAttention}<span class="attention-chip">Needs attention</span>{/if}
            {#if deliveryLabel(pane.agent)}<span class="delivery-chip" role="status">{deliveryLabel(pane.agent)}</span>{/if}
          {/if}
          <div class="terminal-meta">
            {#if pane.kind === 'agent'}
              {@const status = serdeEnumVariantName(pane.agent.status)}
              {@const waiting = typeof pane.agent.status === 'object' && 'WaitingForInput' in pane.agent.status}
              {@const failed = status === 'Error'}
              {@const stopped = typeof pane.agent.status === 'object' && 'WaitingForInput' in pane.agent.status && pane.agent.status.WaitingForInput === 'stopped_by_operator'}
              {@const blockedResume = resumeBlockedReason(session, pane.id)}
              {@const resumableStatus = status === 'Completed' || failed || stopped}
              {#if resumableStatus && blockedResume}
                <span class="resume-blocked">{blockedResume}</span>
              {:else if resumableStatus && pane.agent.provider_session_id}
                <button type="button" class="lattice-btn lattice-btn--ghost"
                  aria-label={`Resume ${pane.title}`} disabled={resumingAgents[pane.id]}
                  onmousedown={(event) => event.preventDefault()}
                  onclick={(event) => resumeAgent(event, pane.id)}>
                  {resumingAgents[pane.id] ? 'Resuming…' : 'Resume'}
                </button>
              {/if}
              {#if resumeErrors[pane.id]}<span role="alert">{resumeErrors[pane.id]}</span>{/if}
              {#if chatEligible}<PaneToggle {hasTerminal} mode={chatShowing ? mode === 'raw' ? 'raw' : 'chat' : 'terminal'} onchange={(next) => { paneModes[pane.id] = next; }} />{/if}
              <span class="cli-badge">{pane.agent.config?.cli || 'unknown'}</span>
              <span class="pane-status-text">{status ?? 'Unknown'}</span>
              <span
                class={`status-indicator status-badge ${waiting ? 'status-warning lattice-motion-status-pulse' : failed ? 'status-error pulse-error' : status === 'Running' ? 'status-running' : status === 'Completed' ? 'status-success' : 'status-queued'}`}
              >
                {#if status === 'Running'}
                  █
                {:else if waiting}
                  <Hourglass size={10} weight="light" />
                {:else if status === 'Completed'}
                  <Check size={10} weight="light" />
                {:else if failed}
                  <X size={10} weight="light" />
                {:else}
                  <Circle size={10} weight="light" />
                {/if}
              </span>
            {:else}
              <span class="cli-badge">{pane.shell}</span>
              <span class="cwd-label" title={`${pane.cwd} · opened ${pane.createdAt}`}>{pane.cwd}</span>
              <button
                type="button"
                class="pane-control lattice-btn lattice-btn--ghost lattice-btn--danger lattice-btn--icon"
                aria-label={`Close ${pane.title}`}
                title="Close scratch terminal"
                disabled={openingScratchId === pane.id}
                onclick={(event) => closeScratchTerminal(event, pane)}
              >
                <X size={13} weight="light" />
              </button>
            {/if}
            {#if !focusMode}
              <button
                type="button"
                class="pane-control lattice-btn lattice-btn--ghost lattice-btn--icon"
                aria-label={pane.id === localMaximizedId ? `Restore ${pane.title}` : `Maximize ${pane.title}`}
                title={pane.id === localMaximizedId ? 'Restore terminal (Esc)' : 'Maximize terminal'}
                onclick={(event) => toggleMaximized(event, pane)}
              >
                {#if pane.id === localMaximizedId}
                  <ArrowsIn size={13} weight="light" />
                {:else}
                  <ArrowsOut size={13} weight="light" />
                {/if}
              </button>
            {/if}
          </div>
        </div>
        {#if focusMode && visible && pane.kind === 'agent' && (pane.id === localMaximizedId || chatEligible || startupExitCode(pane.agent) !== null)}
          <div class="focused-agent-info">
            <AgentActivityLine agentId={pane.id} />
            <AttentionCard sessionId={session.id} agent={pane.agent} {hasTerminal} {flushState} chatAvailable={chatEligible}
              {eventAttention} {feedError}
              onOpen={chatEligible ? () => { paneModes[pane.id] = hasTerminal ? 'terminal' : 'raw'; } : undefined} />
          </div>
        {/if}
        <div class="terminal-container" class:non-chat-attention={gridAttention && !chatEligible && hasTerminal}>
          {#if gridAttention && pane.kind === 'agent'}
            <div class="agent-info">
              <AttentionCard sessionId={session.id} agent={pane.agent} {hasTerminal} {flushState} chatAvailable={chatEligible}
                {eventAttention} {feedError}
                onOpen={chatEligible ? () => { paneModes[pane.id] = hasTerminal ? 'terminal' : 'raw'; } : undefined} />
            </div>
          {/if}
          {#if hasTerminal}
            <div class="terminal-layer" class:chat-hidden={chatShowing} inert={chatShowing} aria-hidden={chatShowing}
              onfocusin={() => { terminalFocusId = pane.id; }}
              onfocusout={(event) => { if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) { if (terminalFocusId === pane.id) terminalFocusId = null; } }}>
              <Terminal
                agentId={pane.id}
                isAgent={pane.kind === 'agent'}
                isFocused={visible && focused && (!chatEligible || (!chatShowing && terminalFocusId === pane.id))}
                isVisible={visible && (localMaximizedId === null || pane.id === localMaximizedId)}
                layoutRevision={localMaximizedId}
                onReady={pane.kind === 'scratch' ? () => markTerminalReady(pane.id) : undefined}
                onStatusChange={pane.kind === 'scratch' ? (status) => handlePaneStatus(pane, status) : undefined}
              />
            </div>
          {/if}
          {#if chatEligible}
            <div class="chat-layer" class:pane-hidden={!chatShowing} inert={!chatShowing} aria-hidden={!chatShowing}>
              <ChatPane sessionId={session.id} agentId={pane.id} events={history} composer={focusMode} variant={focusMode ? 'focus' : 'column'} bind:draft={() => drafts[`${session.id}:${pane.id}`] ?? '', (value) => { drafts = { ...drafts, [`${session.id}:${pane.id}`]: value }; }} raw={mode === 'raw'} visible={visible && chatShowing && (localMaximizedId === null || pane.id === localMaximizedId)} />
            </div>
          {/if}
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
  {#if !focusMode && visible}
    <CommandBar sessionId={session.id} agents={panes.filter((pane): pane is AgentTerminalPane => pane.kind === 'agent').map(pane => pane.agent)} bind:targetId={commandTargetId} bind:drafts ondelivery={(sid, id, state) => { delivery[`${sid}:${id}`] = state; }} />
  {/if}
</div>

<style>
  .terminal-workspace {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    min-height: 0;
    min-width: 0;
  }

  .terminal-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 4px 8px;
  }

  .toolbar-label {
    color: var(--text-muted);
    font-size: 10px;
    font-weight: 700;
  }

  .scratch-controls {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .shell-picker :global(.lattice-input), .pane-picker :global(.lattice-input) {
    height: 28px;
    padding: 0 24px 0 8px;
    font-size: 10px;
  }

  .pane-picker {
    min-width: 0;
  }

  .pane-picker select {
    max-width: 100%;
    text-overflow: ellipsis;
  }

  .focused-agent-info {
    flex: 0 0 auto;
    min-width: 0;
    padding: var(--space-2) var(--space-3);
  }

  /* Share the content's grid cell so this bounded overlay never shrinks the composer. */
  .agent-info {
    grid-area: 1 / 1;
    align-self: start;
    z-index: 1;
    min-width: 0;
    min-height: 0;
    max-height: min(7rem, 35%);
    overflow: auto;
    padding: var(--space-1) var(--space-2);
    background: var(--surface);
  }

  .agent-info :global(.attention) { padding: var(--space-2); }

  .shell-picker select:disabled {
    opacity: 0.5;
  }

  .scratch-error {
    margin: 0 4px 6px;
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

  .terminal-grid.ops-wall {
    grid-template-columns: var(--wall-tracks);
    grid-template-rows: minmax(0, 1fr);
    overflow-x: auto;
    overflow-y: hidden;
    isolation: isolate;
    scroll-padding-left: var(--pin-space);
  }
  .ops-wall > .pinned-column { position: sticky; left: var(--pin-left); z-index: 6; }
  .ops-wall:not(.has-maximized) > .terminal-item:not(.pinned-column) { clip-path: inset(0 0 0 var(--wall-clip, 0px)); }
  .ops-wall > .terminal-item.lattice-panel--active { z-index: 5; }
  .ops-wall > .pinned-column.lattice-panel--active { z-index: 7; }
  .ops-wall > .quiet-column .terminal-header { opacity: 0.65; }
  .ops-wall > .resource-match { outline: 2px solid var(--accent); outline-offset: -2px; }
  .delivery-chip { color: var(--text-muted); font-size: var(--text-micro); }
  .resume-blocked { max-width: 18rem; color: var(--status-warning); font-size: var(--text-micro); overflow-wrap: anywhere; }
  .attention-chip { color: var(--status-warning); font-size: var(--text-micro); }
  .ops-wall .terminal-header { flex-wrap: wrap; min-width: 0; }
  .ops-wall .terminal-meta { flex-wrap: wrap; }
  .ops-wall .terminal-pane-focus { flex-basis: 45%; }
  .ops-wall .pane-activity { flex-basis: 40%; }

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
    padding: 0;
    overflow: hidden;
    background: var(--surface);
    border-radius: var(--radius-lg);
    box-shadow: var(--depth-1);
    transition: box-shadow var(--motion-ui);
    min-height: 0;
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
    background: var(--toolbar);
    box-shadow: var(--edge-seam);
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
  }

  .terminal-meta {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .pane-activity {
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }

  .terminal-pane-focus {
    display: flex;
    flex: 1;
    min-width: 0;
    padding: 0;
    color: inherit;
    text-align: left;
  }

  .pane-control {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
  }

  .cli-badge {
    font-size: var(--text-micro);
    padding: 1px 4px;
    background: var(--surface-raised);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    text-transform: lowercase;
  }

  .cwd-label {
    max-width: 180px;
    overflow: hidden;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--text-micro);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status-indicator.status-badge {
    gap: 0;
    min-width: 18px;
    min-height: 18px;
    justify-content: center;
    padding: 1px 3px;
    font-size: 10px;
  }

  .terminal-container {
    flex: 1;
    min-height: 0;
    min-width: 0;
    display: grid;
    grid-template: minmax(0, 1fr) / minmax(0, 1fr);
    overflow: hidden;
  }

  .terminal-layer, .chat-layer { grid-area: 1 / 1; min-width: 0; min-height: 0; }
  /* Non-chat PTYs have no composer: reserve notice height above their live output. */
  .terminal-container.non-chat-attention {
    grid-template-rows: minmax(0, min(7rem, 35%)) minmax(0, 1fr);
  }
  .non-chat-attention > .agent-info {
    grid-area: 1 / 1;
    align-self: stretch;
    max-height: none;
  }
  .non-chat-attention > .terminal-layer { grid-area: 2 / 1; }
  /* Keep xterm's initial fit measurable and move its native drop rectangle offscreen. */
  .terminal-layer.chat-hidden { visibility: hidden; pointer-events: none; transform: translateX(-200vw); }
  .chat-layer.pane-hidden { visibility: hidden; pointer-events: none; }
  .pane-status-text { font-size: var(--text-small); color: var(--text-muted); }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    color: var(--text-muted);
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
  }
  .ops-wall.has-maximized > .terminal-item { position: absolute; }
</style>
