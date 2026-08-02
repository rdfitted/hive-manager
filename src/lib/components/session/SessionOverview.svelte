<script lang="ts">
    import { onDestroy } from 'svelte';
    import { ui } from '../../stores/ui';
    import { cells } from '../../stores/cells';
    import { agents } from '../../stores/agents';
    import { events } from '../../stores/events';
    import { conversationStore } from '../../stores/conversations';
    import { activeSession, activeAgents } from '../../stores/sessions';
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
                    <div class="terminal-panel" class:hidden={activeView !== 'terminal'}>
                        <TerminalGrid
                            agents={$activeAgents}
                            focusedAgentId={terminalAgentId}
                            onSelect={selectTerminalAgent}
                        />
                    </div>
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
