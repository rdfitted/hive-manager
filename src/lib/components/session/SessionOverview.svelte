<script lang="ts">
    import { onDestroy } from 'svelte';
    import { ui } from '../../stores/ui';
    import { cells } from '../../stores/cells';
    import { agents } from '../../stores/agents';
    import { events } from '../../stores/events';
    import { conversationStore } from '../../stores/conversations';
    import { activeSession, serdeEnumVariantName, sessions } from '../../stores/sessions';
    import { TERMINAL_SESSION_STATES, terminalPool } from '../../stores/terminalPool';
    import SessionHeader from './SessionHeader.svelte';
    import TerminalGrid from '../TerminalGrid.svelte';
    import TimelineView from '../timeline/TimelineView.svelte';
    import ReplayView from '../replay/ReplayView.svelte';
    import ReplayControls from '../replay/ReplayControls.svelte';
    import ArtifactBrowser from '../artifacts/ArtifactBrowser.svelte';

    type SessionView = 'terminal' | 'observability' | 'artifacts';
    let activeView: SessionView = $state('terminal');
    const sessionId = $derived($activeSession?.id);
    const terminalAgentId = $derived($ui.selectedAgentId || $ui.focusedAgentId);

    function selectTerminalAgent(id: string) {
        ui.setFocusedAgent(id);
        ui.setSelectedAgent(id);
    }

    const sessionNotFound = $derived($cells.sessionNotFound);
    let connectedSessionId: string | null = null;

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
    });
</script>

<div class="session-overview">
    <header>
        <SessionHeader />
    </header>

    <main>
        {#if sessionNotFound}
            <div class="session-not-found">
                <h2>Session Not Found</h2>
                <p>The session may have been deleted or never existed.</p>
                <p>ID: {sessionId}</p>
            </div>
        {:else}
            <div class="terminal-section lattice-panel">
                <div class="terminal-controls lattice-forced-colors-boundary">
                    <div class="tab-bar" role="tablist" aria-label="Session view">
                        <button type="button" role="tab" class="lattice-tab" class:lattice-tab--active={activeView === 'terminal'} aria-selected={activeView === 'terminal'} onclick={() => activeView = 'terminal'}>Terminal</button>
                        <button type="button" role="tab" class="lattice-tab" class:lattice-tab--active={activeView === 'observability'} aria-selected={activeView === 'observability'} onclick={() => activeView = 'observability'}>Observability</button>
                        <button type="button" role="tab" class="lattice-tab" class:lattice-tab--active={activeView === 'artifacts'} aria-selected={activeView === 'artifacts'} onclick={() => activeView = 'artifacts'}>Artifacts</button>
                    </div>
                </div>
                <div class="terminal-wrapper">
                    {#each pooledSessions as pooled (pooled.id)}
                        {@const isActiveGrid = pooled.id === sessionId}
                        <!-- Pooled grids stay mounted but hidden so a session switch shows
                             the terminals exactly as the operator left them (#286). -->
                        <div class="terminal-panel" class:hidden={activeView !== 'terminal' || !isActiveGrid}>
                            <TerminalGrid
                                session={pooled}
                                visible={activeView === 'terminal' && isActiveGrid}
                                focusedAgentId={isActiveGrid ? terminalAgentId : null}
                                onSelect={selectTerminalAgent}
                            />
                        </div>
                    {/each}
                    {#if activeView === 'observability'}
                        <div class="observability-container">
                            <div class="obs-main">
                                <div class="obs-timeline lattice-forced-colors-boundary">
                                    <TimelineView />
                                </div>
                                <div class="obs-replay lattice-forced-colors-boundary">
                                    <ReplayView />
                                </div>
                            </div>
                            <ReplayControls />
                        </div>
                    {:else if activeView === 'artifacts'}
                        <ArtifactBrowser />
                    {/if}
                </div>
            </div>
        {/if}
    </main>
</div>

<style>
    .session-overview {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--bg-void);
        color: var(--text-primary);
    }

    header {
        flex: 0 0 auto;
    }

    main {
        flex: 1;
        display: flex;
        flex-direction: column;
        overflow: hidden;
    }

    .session-not-found {
        flex: 1;
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        background: var(--bg-void);
        color: var(--text-primary);
        text-align: center;
        padding: 2rem;
    }

    .session-not-found h2 {
        color: var(--status-error);
        margin-bottom: 1rem;
    }

    .session-not-found p {
        color: var(--text-secondary);
        margin-bottom: 0.5rem;
    }

    .terminal-section {
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: column;
    }

    .terminal-controls {
        padding: 0 12px;
        display: flex;
        justify-content: space-between;
        align-items: center;
        box-shadow: var(--edge-seam);
    }

    .tab-bar {
        display: flex;
        gap: 2px;
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

    .observability-container {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--bg-void);
    }

    .obs-main {
        flex: 1;
        display: grid;
        grid-template-columns: 1fr 1fr;
        overflow: hidden;
    }

    .obs-timeline, .obs-replay {
        overflow: hidden;
        box-shadow: inset -1px 0 0 rgba(0, 0, 0, 0.45);
    }
</style>
