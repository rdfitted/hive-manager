<script lang="ts">
    import type { AgentStatus } from '../../types/domain';

    export let status: AgentStatus;

    $: color = {
        'queued': '#6b7280',
        'launching': '#3b82f6',
        'running': '#10b981',
        'completed': '#10b981',
        'waiting_input': '#f59e0b',
        'failed': '#ef4444',
        'killed': '#6b7280'
    }[status] || '#6b7280';
</script>

<div class="status-badge" style="--status-color: {color}">
    <span class="dot"></span>
    <span class="label">{status.replace('_', ' ')}</span>
</div>

<style>
    .status-badge {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        font-size: 11px;
        color: var(--status-color);
        font-weight: 500;
        text-transform: capitalize;
    }

    .dot {
        width: 8px;
        height: 8px;
        background: var(--status-color);
        border-radius: 50%;
        box-shadow: 0 0 8px var(--status-color);
    }
</style>
