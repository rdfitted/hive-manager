<script lang="ts">
    import { onDestroy } from 'svelte';
    import { ui } from '../../stores/ui';
    import { cells } from '../../stores/cells';
    import { events } from '../../stores/events';
    import { activeSession, activeAgents, serdeEnumVariantName } from '../../stores/sessions';
    import SessionHeader from './SessionHeader.svelte';
    import CellGrid from '../cell/CellGrid.svelte';
    import Terminal from '../Terminal.svelte';
    import TimelineView from '../timeline/TimelineView.svelte';
    import ReplayView from '../replay/ReplayView.svelte';
    import ReplayControls from '../replay/ReplayControls.svelte';
    import ArtifactBrowser from '../artifacts/ArtifactBrowser.svelte';
    import { Hourglass, Check, Circle } from 'phosphor-svelte';

    type SessionView = 'terminal' | 'observability' | 'artifacts';
    let activeView: SessionView = $state('terminal');
    const sessionId = $derived($activeSession?.id);
    const terminalMaximized = $derived($ui.terminalMaximized);
    const terminalAgentId = $derived($ui.selectedAgentId || $ui.focusedAgentId);
    
    const focusedAgent = $derived($activeAgents.find(a => a.id === terminalAgentId));
    const roleName = $derived(focusedAgent ? (focusedAgent.config?.label ||
              (focusedAgent.role === 'Queen' ? 'Queen' :
               focusedAgent.role === 'Evaluator' ? 'Evaluator' :
               typeof focusedAgent.role === 'object' && 'Planner' in focusedAgent.role ? `Planner ${focusedAgent.role.Planner.index}` :
               typeof focusedAgent.role === 'object' && 'Worker' in focusedAgent.role ? `Worker ${focusedAgent.role.Worker.index}` :
               typeof focusedAgent.role === 'object' && 'QaWorker' in focusedAgent.role ? `QA Worker ${focusedAgent.role.QaWorker.index}` :
               'Agent')) : '');

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
        if (!sessionId) return;
        pollTimeout = setTimeout(() => {
            if (sessionId) {
                cells.fetchCells(sessionId);
                schedulePoll();
            }
        }, 10000);
    }

    $effect(() => {
        if (sessionId && sessionId !== connectedSessionId) {
            connectedSessionId = sessionId;
            cells.fetchCells(sessionId);
            cells.setExternalRefreshHandler(() => {
                schedulePoll();
            });
            schedulePoll();
            events.disconnect();
            events.connect(sessionId);
        } else if (sessionId) {
            cells.setExternalRefreshHandler(() => {
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
                    {#each $activeAgents as agent (agent.id)}
                        {@const isVisible = agent.id === terminalAgentId}
                        {@const agentRoleName = agent.config?.label ||
                            (agent.role === 'Queen' ? 'Queen' :
                             agent.role === 'Evaluator' ? 'Evaluator' :
                             typeof agent.role === 'object' && 'Planner' in agent.role ? `Planner ${agent.role.Planner.index}` :
                             typeof agent.role === 'object' && 'Worker' in agent.role ? `Worker ${agent.role.Worker.index}` :
                             typeof agent.role === 'object' && 'QaWorker' in agent.role ? `QA Worker ${agent.role.QaWorker.index}` :
                             'Agent')}
                        <div class="agent-terminal-view" class:hidden={!isVisible}>
                            <div
                                class="terminal-header"
                                style:border-top={$activeSession?.color ? `3px solid ${$activeSession.color}` : 'none'}
                            >
                                <span class="terminal-title">{agentRoleName}</span>
                                <div class="terminal-meta">
                                    <span class="cli-badge">{agent.config?.cli || 'unknown'}</span>
                                    <span class="terminal-status" 
                                        class:running={agent.status === 'Running'} 
                                        class:waiting={typeof agent.status === 'object' && 'WaitingForInput' in agent.status} 
                                        class:completed={agent.status === 'Completed'}
                                    >
                                        {#if agent.status === 'Running'}
                                            █
                                        {:else if typeof agent.status === 'object' && 'WaitingForInput' in agent.status}
                                            <Hourglass size={10} weight="light" />
                                        {:else if agent.status === 'Completed'}
                                            <Check size={10} weight="light" />
                                        {:else}
                                            <Circle size={10} weight="light" />
                                        {/if}
                                    </span>

                                </div>
                            </div>
                            <div class="terminal-container">
                                <Terminal agentId={agent.id} isFocused={isVisible} />
                            </div>
                        </div>
                    {/each}
                    {#if $activeAgents.length === 0}
                        <div class="no-agent-selected">
                            No agents in this session
                        </div>
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
        height: 100%;
        background: var(--color-bg);
        color: var(--color-text);
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
        flex: 0 0 45%;
        background: var(--color-surface);
        border-top: 1px solid var(--color-border);
        display: flex;
        flex-direction: column;
        transition: flex 0.3s cubic-bezier(0.4, 0, 0.2, 1);
    }

    .session-overview.terminal-maximized .terminal-section {
        flex: 0 0 90%;
    }

    .terminal-controls {
        padding: 0 12px;
        background: var(--color-surface);
        display: flex;
        justify-content: space-between;
        align-items: center;
        border-bottom: 1px solid var(--color-border);
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
        color: var(--color-text-muted);
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        cursor: pointer;
        transition: all 0.2s;
    }

    .tab-btn:hover {
        color: var(--color-text);
    }

    .tab-btn.active {
        color: var(--color-accent);
        border-bottom-color: var(--color-accent);
    }

    .expand-btn {
        background: transparent;
        border: none;
        color: var(--color-text-muted);
        font-size: 10px;
        cursor: pointer;
        text-transform: uppercase;
        font-weight: bold;
    }

    .expand-btn:hover {
        color: var(--color-text);
    }

    .terminal-wrapper {
        flex: 1;
        overflow: hidden;
        position: relative;
    }

    .terminal-panel {
        height: 100%;
    }

    .agent-terminal-view {
        display: flex;
        flex-direction: column;
        height: 100%;
    }

    .agent-terminal-view.hidden {
        display: none;
    }

    .terminal-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 6px 12px;
        background: var(--color-bg);
        border-bottom: 1px solid var(--color-border);
    }

    .terminal-title {
        font-size: 11px;
        font-weight: 600;
        color: var(--color-text);
        text-transform: uppercase;
        letter-spacing: 0.5px;
    }

    .terminal-meta {
        display: flex;
        align-items: center;
        gap: 8px;
    }

    .cli-badge {
        font-size: 9px;
        padding: 1px 5px;
        background: var(--color-border);
        border-radius: 3px;
        color: var(--color-text-muted);
        text-transform: lowercase;
    }

    .terminal-status {
        font-size: 10px;
    }

    .terminal-status.running { color: var(--color-running); }
    .terminal-status.waiting { color: var(--color-warning); }
    .terminal-status.completed { color: var(--color-success); }

    .terminal-container {
        flex: 1;
        min-height: 0;
        background: var(--bg-void);
    }

    .no-agent-selected {
        height: 100%;
        display: flex;
        align-items: center;
        justify-content: center;
        color: var(--color-text-muted);
        font-size: 13px;
    }

    .terminal-panel.hidden {
        display: none;
    }

    .observability-container {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--color-bg);
    }

    .obs-main {
        flex: 1;
        display: grid;
        grid-template-columns: 1fr 1fr;
        overflow: hidden;
    }

    .obs-timeline, .obs-replay {
        overflow: hidden;
        border-right: 1px solid var(--color-border);
    }
</style>
