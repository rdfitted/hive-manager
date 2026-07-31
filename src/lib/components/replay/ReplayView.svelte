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

    function getStatusClass(status: string): string {
        switch (status?.toLowerCase()) {
            case 'running':
            case 'active':
            case 'in_progress':
                return 'status-running';
            case 'completed':
            case 'complete':
            case 'succeeded':
                return 'status-success';
            case 'failed':
            case 'error':
                return 'status-error';
            case 'blocked':
                return 'status-blocked';
            case 'canceled':
            case 'cancelled':
                return 'status-canceled';
            default:
                return 'status-queued';
        }
    }
</script>

<div class="replay-view lattice-scroll-content">
    <div class="state-header">
        Session Status: <span class="status-badge {getStatusClass(state.sessionStatus)}">{state.sessionStatus}</span>
    </div>

    <div class="state-grid">
        <div class="state-section">
            <h4>Cells</h4>
            <div class="grid">
                {#each Object.entries(state.cells) as [id, status]}
                    <div class="state-chip">
                        <span class="label">{id.substring(0, 8)}</span>
                        <span class="status-badge {getStatusClass(status)}">{status}</span>
                    </div>
                {/each}
            </div>
        </div>

        <div class="state-section">
            <h4>Agents</h4>
            <div class="grid">
                {#each Object.entries(state.agents) as [id, status]}
                    <div class="state-chip">
                        <span class="label">{id.substring(0, 8)}</span>
                        <span class="status-badge {getStatusClass(status)}">{status}</span>
                    </div>
                {/each}
            </div>
        </div>
    </div>
</div>

<style>
    .replay-view {
        padding: 16px;
        background: var(--bg-void);
        height: 100%;
        overflow-y: auto;
        font-family: var(--font-mono);
    }

    .state-header {
        font-size: 1.1rem;
        font-weight: bold;
        margin-bottom: 20px;
        color: var(--text-primary);
    }

    .state-grid {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 24px;
    }

    .state-section h4 {
        margin: 0 0 12px 0;
        color: var(--text-secondary);
        text-transform: uppercase;
        font-size: 0.75rem;
        border-bottom: 1px solid var(--border-structural);
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
        /* Replay entity rows are compact state records, not structural panels. */
        background: var(--bg-surface);
        border: 1px solid var(--border-structural);
        border-radius: var(--radius-sm);
        font-size: 0.85rem;
    }

    .label {
        color: var(--accent-cyan);
        font-weight: bold;
    }

    .state-chip :global(.status-badge) {
        margin-left: auto;
    }
</style>
