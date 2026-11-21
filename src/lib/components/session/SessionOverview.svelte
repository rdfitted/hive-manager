<script lang="ts">
    import { onDestroy } from 'svelte';
    import { ui } from '../../stores/ui';
    import { cells } from '../../stores/cells';
    import { agents } from '../../stores/agents';
    import { events } from '../../stores/events';
    import { agentEvents } from '../../stores/agentEvents';
    import { conversationStore } from '../../stores/conversations';
    import { activeSession, serdeEnumVariantName, sessions } from '../../stores/sessions';
    import { TERMINAL_SESSION_STATES, terminalPool } from '../../stores/terminalPool';
    import OpsWall from '../opswall/OpsWall.svelte';
    import type { ComponentProps } from 'svelte';
    import TerminalGrid from '../TerminalGrid.svelte';
    const sessionId = $derived($activeSession?.id);
    const terminalAgentId = $derived($ui.focusedAgentId || $ui.selectedAgentId);

    type GridProps = ComponentProps<typeof TerminalGrid>;
    type Seats = Parameters<NonNullable<GridProps['onSeats']>>[0];
    const emptySeats: Seats = { attention: new Set(), stale: new Set() };
    let reportedSeats = $state.raw<{ sessionId: string; seats: Seats } | null>(null);
    const seats = $derived(reportedSeats && reportedSeats.sessionId === sessionId ? reportedSeats.seats : emptySeats);
    let filterSelection = $state<{ sessionId: string; filter: GridProps['resourceFilter'] } | null>(null);
    const resourceFilter = $derived(filterSelection && filterSelection.sessionId === sessionId ? filterSelection.filter : null);
    let revealNonce = 0;
    let reveal = $state<{ sessionId: string; request: NonNullable<GridProps['revealRequest']> } | null>(null);
    const revealRequest = $derived(reveal && reveal.sessionId === sessionId ? reveal.request : null);

    function revealAgent(agentId: string) {
        if (sessionId) reveal = { sessionId, request: { agentId, nonce: ++revealNonce } };
    }

    function filterResources(filter: string | null) {
        if (!sessionId || (filter !== null && filter !== 'attention' && filter !== 'waiting' && filter !== 'stale')) return;
        filterSelection = { sessionId, filter };
        const first = filter === 'attention' ? seats.attention.values().next().value
            : filter === 'stale' ? seats.stale.values().next().value
            : filter === 'waiting' ? $activeSession?.agents.find(agent => serdeEnumVariantName(agent.status) === 'WaitingForInput')?.id
            : undefined;
        if (first) revealAgent(first);
    }

    function selectTerminalAgent(id: string) {
        ui.setFocusedAgent(id);
        ui.setSelectedAgent(id);
    }

    const sessionNotFound = $derived($cells.sessionNotFound);
    let connectedSessionId: string | null = null;

    $effect(() => {
        agentEvents.setSessionId(sessionNotFound ? null : sessionId ?? null);
    });

    // Keep-alive pool (#286): the active session plus the most recently visited ones keep
    // their terminal grids mounted (hidden), so switching back shows the panes exactly as
    // they were. The active session always leads so its grid exists on the same tick.
    const POOL_SWEEP_INTERVAL_MS = 30_000;
    const pooledSessions = $derived.by(() => {
        const byId = new Map($sessions.sessions.map((session) => [session.id, session]));
        const ids = [
            ...($activeSession ? [$activeSession.id] : []),
            ...$terminalPool.map((entry) => entry.sessionId),
        ];
        const seen = new Set<string>();
        const result = [];
        for (const id of ids) {
            if (seen.has(id)) continue;
            seen.add(id);
            const session = byId.get(id);
            if (session) result.push(session);
        }
        return result;
    });

    $effect(() => {
        if (sessionId) terminalPool.touch(sessionId);
    });

    function sweepPool() {
        const known = new Map($sessions.sessions.map((session) => [session.id, session]));
        terminalPool.sweep(sessionId ?? null, {
            exists: (id) => known.has(id),
            isTerminal: (id) =>
                TERMINAL_SESSION_STATES.has(serdeEnumVariantName(known.get(id)?.state) ?? ''),
        });
    }
    const poolSweep = setInterval(sweepPool, POOL_SWEEP_INTERVAL_MS);
    let pollTimeout: ReturnType<typeof setTimeout> | null = null;

    function clearPollTimeout() {
        if (pollTimeout) {
            clearTimeout(pollTimeout);
            pollTimeout = null;
        }
    }

    function schedulePoll() {
        clearPollTimeout();
        if (!sessionId || sessionNotFound) return;

        pollTimeout = setTimeout(async () => {
            if (!sessionId || sessionNotFound) {
                return;
            }

            try {
                await fetchCellsAndAgents(sessionId);
            } finally {
                if (!sessionNotFound) {
                    schedulePoll();
                }
            }
        }, 10000);
    }

    async function fetchCellsAndAgents(sid: string) {
        await cells.fetchCells(sid);
        const cellIds = Object.keys($cells.cells);
        await Promise.all(cellIds.map(cid => agents.fetchAgents(sid, cid)));
        // Also poll for messages in the selected conversation
        await conversationStore.pollMessages();
    }

    $effect(() => {
        if (sessionId && sessionId !== connectedSessionId) {
            connectedSessionId = sessionId;
            fetchCellsAndAgents(sessionId);
            cells.setExternalRefreshHandler(() => {
                // Immediate refresh on external signal (e.g. Tauri event)
                void fetchCellsAndAgents(sessionId);
                schedulePoll();
            });
            schedulePoll();
            events.disconnect();
            events.connect(sessionId);
        } else if (sessionId) {
            cells.setExternalRefreshHandler(() => {
                void fetchCellsAndAgents(sessionId);
                schedulePoll();
            });
        }
    });

    $effect(() => {
        if (!sessionId && connectedSessionId) {
            connectedSessionId = null;
            clearPollTimeout();
            cells.setExternalRefreshHandler(null);
            events.disconnect();
        }
    });

    onDestroy(() => {
        clearInterval(poolSweep);
        clearPollTimeout();
        cells.setExternalRefreshHandler(null);
        events.disconnect();
        agentEvents.disconnect();
    });
</script>

<div class="session-overview">
    <main>
        {#if sessionNotFound}
            <div class="session-not-found">
                <h2>Session Not Found</h2>
                <p>The session may have been deleted or never existed.</p>
                <p>ID: {sessionId}</p>
            </div>
        {:else}
            <div class="terminal-section lattice-panel">
                <div class="terminal-wrapper">
                    {#each pooledSessions as pooled (pooled.id)}
                        {@const isActiveGrid = pooled.id === sessionId}
                        <!-- Pooled grids stay mounted but hidden so a session switch shows
                             the terminals exactly as the operator left them (#286). -->
                        <div class="terminal-panel" class:hidden={!isActiveGrid}>
                            <TerminalGrid
                                session={pooled}
                                visible={isActiveGrid}
                                focusedAgentId={isActiveGrid ? terminalAgentId : null}
                                onSelect={selectTerminalAgent}
                                resourceFilter={isActiveGrid ? resourceFilter : undefined}
                                revealRequest={isActiveGrid ? revealRequest : undefined}
                                onSeats={isActiveGrid ? (seats) => { reportedSeats = { sessionId: pooled.id, seats }; } : undefined}
                            />
                        </div>
                    {/each}
                </div>
            </div>
            {#if $activeSession}
                <div class="session-ops-band">
                    <OpsWall band session={$activeSession} attention={seats.attention} stale={seats.stale}
                        filter={resourceFilter ?? null} onfilter={filterResources} onselect={revealAgent} />
                </div>
            {/if}
        {/if}
    </main>
</div>

<style>
  .session-overview {
      display: flex;
      flex-direction: column;
      height: 100%;
      color: var(--ink);
  }

  main {
      flex: 1;
      display: flex;
      flex-direction: column;
      gap: var(--space-3);
      min-height: 0;
  }

  .session-not-found {
      flex: 1;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      color: var(--ink);
      text-align: center;
      padding: 2rem;
  }

  .session-not-found h2 {
      color: var(--status-error);
      margin-bottom: 1rem;
  }

  .session-not-found p {
      color: var(--text-muted);
      margin-bottom: 0.5rem;
  }

  .session-ops-band { flex: 0 0 auto; min-width: 0; }

  /* The stage is not a card: panes sit straight on the canvas. */
  .terminal-section {
      flex: 1;
      min-height: 0;
      display: flex;
      flex-direction: column;
      padding: 0;
      border-radius: 0;
      background: transparent;
      box-shadow: none;
  }

  .terminal-wrapper {
      flex: 1;
      overflow: hidden;
      position: relative;
  }

  .terminal-panel {
      height: 100%;
  }

  .terminal-panel.hidden {
      display: none;
  }

  @media (forced-colors: active) {
      .terminal-section { border: 0 !important; }
  }
</style>
