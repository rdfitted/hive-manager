<script lang="ts">
    import type { AgentStatus } from '../../types/domain';

    export let status: AgentStatus;

    $: color = {
        'queued': 'var(--status-queued)',
        'launching': 'var(--status-running)',
        'running': 'var(--status-running)',
        'completed': 'var(--status-success)',
        'waiting_input': 'var(--status-warning)',
        'failed': 'var(--status-error)',
        'killed': 'var(--status-queued)'
    }[status] || 'var(--status-queued)';
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
