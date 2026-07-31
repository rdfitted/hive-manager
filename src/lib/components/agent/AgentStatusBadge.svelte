<script lang="ts">
    import type { AgentStatus } from '../../types/domain';

    export let status: AgentStatus;

    function getStatusClass(value: AgentStatus): string {
        if (value === 'launching' || value === 'running') return 'status-running';
        if (value === 'completed') return 'status-success';
        if (value === 'waiting_input') return 'status-warning';
        if (value === 'failed') return 'status-error';
        return 'status-queued';
    }
</script>

<div class="status-badge {getStatusClass(status)}">
    <span class="dot"></span>
    <span class="label">{status.replace('_', ' ')}</span>
</div>

<style>
    .dot {
        width: 8px;
        height: 8px;
        background: currentColor;
        border-radius: 50%;
        box-shadow: 0 0 8px currentColor;
    }
</style>
