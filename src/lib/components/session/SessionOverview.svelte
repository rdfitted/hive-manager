<script lang="ts">
    import { onDestroy } from 'svelte';
    import { ui } from '../../stores/ui';
    import { cells } from '../../stores/cells';
    import { events } from '../../stores/events';
    import { activeSession } from '../../stores/sessions';
    import SessionHeader from './SessionHeader.svelte';
    import CellGrid from '../cell/CellGrid.svelte';
    import Terminal from '../Terminal.svelte';
    import TimelineView from '../timeline/TimelineView.svelte';
    import ReplayView from '../replay/ReplayView.svelte';
    import ReplayControls from '../replay/ReplayControls.svelte';
    import ArtifactBrowser from '../artifacts/ArtifactBrowser.svelte';

    type SessionView = 'terminal' | 'observability' | 'artifacts';
    let activeView: SessionView = $state('terminal');
    const sessionId = $derived($activeSession?.id);
    const terminalMaximized = $derived($ui.terminalMaximized);
    const terminalAgentId = $derived($ui.selectedAgentId || $ui.focusedAgentId);

    let connectedSessionId: string | null = null;

    $effect(() => {
        if (sessionId && sessionId !== connectedSessionId) {
            connectedSessionId = sessionId;
            cells.fetchCells(sessionId);
            events.disconnect();
            events.connect(sessionId);
        }
    });

    $effect(() => {
        if (!sessionId && connectedSessionId) {
            connectedSessionId = null;
            events.disconnect();
        }
    });

    onDestroy(() => {
        events.disconnect();
    });

    function toggleTerminal() {
        ui.setTerminalMaximized(!terminalMaximized);
    }
</script>

<div class="session-overview" class:terminal-maximized={terminalMaximized}>
    <header>
        <SessionHeader />
    </header>

    <main>
        <div class="grid-section">
            <CellGrid />
        </div>

        <div class="terminal-section">
            <div class="terminal-controls">
                <div class="tab-bar">
                    <button class="tab-btn" class:active={activeView === 'terminal'} onclick={() => activeView = 'terminal'}>Terminal</button>
                    <button class="tab-btn" class:active={activeView === 'observability'} onclick={() => activeView = 'observability'}>Observability</button>
                    <button class="tab-btn" class:active={activeView === 'artifacts'} onclick={() => activeView = 'artifacts'}>Artifacts</button>
                </div>
                <button class="expand-btn" onclick={toggleTerminal}>
                    {terminalMaximized ? 'Minimize' : 'Maximize'}
                </button>
            </div>
            <div class="terminal-wrapper">
                <div class="terminal-panel" class:hidden={activeView !== 'terminal'}>
                    {#if terminalAgentId}
                        <Terminal agentId={terminalAgentId} />
                    {/if}
                </div>
                {#if activeView === 'observability'}
                    <div class="observability-container">
                        <div class="obs-main">
                            <div class="obs-timeline">
                                <TimelineView />
                            </div>
                            <div class="obs-replay">
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
    </main>
</div>

<style>
    .session-overview {
        display: flex;
        flex-direction: column;
        height: 100vh;
        background: #000;
        color: #ddd;
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

    .grid-section {
        flex: 1;
        overflow: hidden;
    }

    .terminal-section {
        flex: 0 0 40%;
        background: #050505;
        border-top: 1px solid rgba(255, 255, 255, 0.05);
        display: flex;
        flex-direction: column;
        transition: flex 0.3s cubic-bezier(0.4, 0, 0.2, 1);
    }

    .session-overview.terminal-maximized .terminal-section {
        flex: 0 0 90%;
    }

    .terminal-controls {
        padding: 0 12px;
        background: #111;
        display: flex;
        justify-content: space-between;
        align-items: center;
        border-bottom: 1px solid rgba(255, 255, 255, 0.05);
    }

    .tab-bar {
        display: flex;
        gap: 2px;
    }

    .tab-btn {
        padding: 8px 16px;
        background: transparent;
        border: none;
        border-bottom: 2px solid transparent;
        color: #666;
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        cursor: pointer;
        transition: all 0.2s;
    }

    .tab-btn:hover {
        color: #aaa;
    }

    .tab-btn.active {
        color: var(--color-accent, #7aa2f7);
        border-bottom-color: var(--color-accent, #7aa2f7);
    }

    .expand-btn {
        background: transparent;
        border: none;
        color: #555;
        font-size: 10px;
        cursor: pointer;
        text-transform: uppercase;
        font-weight: bold;
    }

    .expand-btn:hover {
        color: #888;
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
        background: #1a1b26;
    }

    .obs-main {
        flex: 1;
        display: grid;
        grid-template-columns: 1fr 1fr;
        overflow: hidden;
    }

    .obs-timeline, .obs-replay {
        overflow: hidden;
        border-right: 1px solid rgba(255, 255, 255, 0.05);
    }
</style>
