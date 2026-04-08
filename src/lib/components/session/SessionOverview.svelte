<script lang="ts">
    import { onDestroy } from 'svelte';
    import { ui } from '../../stores/ui';
    import { cells } from '../../stores/cells';
    import { events } from '../../stores/events';
    import { activeSession } from '../../stores/sessions';
    import SessionHeader from './SessionHeader.svelte';
    import CellGrid from '../cell/CellGrid.svelte';
    import Terminal from '../Terminal.svelte';

    $: sessionId = $activeSession?.id;
    $: terminalMaximized = $ui.terminalMaximized;
    $: terminalAgentId = $ui.selectedAgentId || $ui.focusedAgentId;
    let connectedSessionId: string | null = null;

    $: if (sessionId && sessionId !== connectedSessionId) {
        connectedSessionId = sessionId;
        cells.fetchCells(sessionId);
        events.disconnect();
        events.connect(sessionId);
    }

    $: if (!sessionId && connectedSessionId) {
        connectedSessionId = null;
        events.disconnect();
    }

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
                <span class="label">Terminal</span>
                <button class="expand-btn" on:click={toggleTerminal}>
                    {terminalMaximized ? 'Minimize' : 'Maximize'}
                </button>
            </div>
            <div class="terminal-wrapper">
                {#if terminalAgentId}
                    <Terminal agentId={terminalAgentId} />
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
        padding: 4px 12px;
        background: #111;
        display: flex;
        justify-content: space-between;
        align-items: center;
        border-bottom: 1px solid rgba(255, 255, 255, 0.05);
    }

    .terminal-controls .label {
        font-size: 10px;
        text-transform: uppercase;
        color: #555;
        font-weight: 700;
        letter-spacing: 0.1em;
    }

    .expand-btn {
        background: transparent;
        border: none;
        color: #888;
        font-size: 10px;
        cursor: pointer;
    }

    .expand-btn:hover {
        color: #fff;
    }

    .terminal-wrapper {
        flex: 1;
        overflow: hidden;
    }
</style>
