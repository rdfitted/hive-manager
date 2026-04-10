<script lang="ts">
    import { eventsAtTimestamp } from '$lib/stores/replay';
    import { activeSession, serdeEnumVariantName } from '$lib/stores/sessions';
    import type { CellStatus, AgentStatus } from '$lib/types/domain';

    interface ReplayedState {
        cells: Record<string, CellStatus>;
        agents: Record<string, AgentStatus>;
        sessionStatus: string;
    }

    $: state = $eventsAtTimestamp.reduce<ReplayedState>((acc, event) => {
        switch (event.event_type) {
            case 'session_status_changed':
                acc.sessionStatus = event.payload.status;
                break;
            case 'cell_status_changed':
                if (event.cell_id) acc.cells[event.cell_id] = event.payload.status;
                break;
            case 'agent_launched':
                if (event.agent_id) acc.agents[event.agent_id] = 'running';
                break;
            case 'agent_completed':
                if (event.agent_id) acc.agents[event.agent_id] = 'completed';
                break;
            case 'agent_failed':
                if (event.agent_id) acc.agents[event.agent_id] = 'failed';
                break;
        }
        return acc;
    }, {
        cells: {} as Record<string, CellStatus>,
        agents: {} as Record<string, AgentStatus>,
        sessionStatus: serdeEnumVariantName($activeSession?.state) || 'unknown'
    } as ReplayedState);

    function getStatusColor(status: string) {
        switch (status?.toLowerCase()) {
            case 'running': return 'var(--color-running)';
            case 'completed': return 'var(--color-success)';
            case 'failed': return 'var(--color-error)';
            default: return 'var(--color-text-muted)';
        }
    }
</script>

<div class="replay-view">
    <div class="state-header">
        Session Status: <span style="color: {getStatusColor(state.sessionStatus)}">{state.sessionStatus}</span>
    </div>

    <div class="state-grid">
        <div class="state-section">
            <h4>Cells</h4>
            <div class="grid">
                {#each Object.entries(state.cells) as [id, status]}
                    <div class="state-chip">
                        <span class="dot" style="background: {getStatusColor(status)}"></span>
                        <span class="label">{id.substring(0, 8)}</span>
                        <span class="status">{status}</span>
                    </div>
                {/each}
            </div>
        </div>

        <div class="state-section">
            <h4>Agents</h4>
            <div class="grid">
                {#each Object.entries(state.agents) as [id, status]}
                    <div class="state-chip">
                        <span class="dot" style="background: {getStatusColor(status)}"></span>
                        <span class="label">{id.substring(0, 8)}</span>
                        <span class="status">{status}</span>
                    </div>
                {/each}
            </div>
        </div>
    </div>
</div>

<style>
    .replay-view {
        padding: 16px;
        background: var(--color-bg);
        height: 100%;
        overflow-y: auto;
        font-family: var(--font-mono);
    }

    .state-header {
        font-size: 1.1rem;
        font-weight: bold;
        margin-bottom: 20px;
        color: var(--color-text);
    }

    .state-grid {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 24px;
    }

    .state-section h4 {
        margin: 0 0 12px 0;
        color: var(--color-text-muted);
        text-transform: uppercase;
        font-size: 0.75rem;
        border-bottom: 1px solid var(--color-border);
        padding-bottom: 4px;
    }

    .grid {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .state-chip {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 6px 12px;
        background: var(--color-surface);
        border: 1px solid var(--color-border);
        border-radius: var(--radius-sm);
        font-size: 0.85rem;
    }

    .dot {
        width: 8px;
        height: 8px;
        border-radius: 50%;
    }

    .label {
        color: var(--color-accent);
        font-weight: bold;
    }

    .status {
        margin-left: auto;
        color: var(--color-text-muted);
        font-size: 0.75rem;
    }
</style>
