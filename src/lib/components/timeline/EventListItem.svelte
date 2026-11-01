<script lang="ts">
    import type { Event } from '$lib/types/domain';
    import { createEventDispatcher } from 'svelte';

    export let event: Event;

    const dispatch = createEventDispatcher();

    function formatTime(ts: string) {
        return new Date(ts).toLocaleTimeString('en-US', { 
            hour12: false, 
            hour: '2-digit', 
            minute: '2-digit', 
            second: '2-digit' 
        });
    }

    function getSeverityColor(severity: string) {
        switch (severity) {
            case 'error': return 'var(--status-error)';
            case 'warning': return 'var(--status-warning)';
            default: return 'var(--accent)';
        }
    }

    function getEventTypeLabel(type: string) {
        return type.replace(/_/g, ' ').toUpperCase();
    }

    function getPayloadExcerpt(payload: any) {
        if (!payload) return '';
        const s = JSON.stringify(payload);
        return s.length > 60 ? s.substring(0, 57) + '...' : s;
    }
</script>

<button type="button" class="event-row lattice-btn lattice-btn--row" on:click={() => dispatch('select', event)}>
    <div class="event-item lattice-forced-colors-boundary">
        <div class="severity-indicator" style="background-color: {getSeverityColor(event.severity)}"></div>
        
        <div class="event-time">{formatTime(event.timestamp)}</div>
        
        <div class="event-type" title={event.event_type}>
            {getEventTypeLabel(event.event_type)}
        </div>

        <div class="event-source">
            {#if event.cell_id}
                <span class="badge cell-badge lattice-forced-colors-boundary">{event.cell_id.substring(0, 8)}</span>
            {/if}
            {#if event.agent_id}
                <span class="badge agent-badge lattice-forced-colors-boundary">{event.agent_id.substring(0, 8)}</span>
            {/if}
        </div>

        <div class="event-payload">
            {getPayloadExcerpt(event.payload)}
        </div>
    </div>
</button>

<style>
    .event-item {
        display: grid;
        grid-template-columns: 4px 80px 140px 120px 1fr;
        gap: 12px;
        align-items: center;
        width: 100%;
        box-shadow: none;
        font-family: var(--font-body);
        font-size: 0.8rem;
    }

    .event-row:nth-child(even) {
        background: var(--surface);
    }

    .severity-indicator {
        height: 100%;
        width: 4px;
        border-radius: var(--radius-md);
    }

    .event-time {
        color: var(--text-muted);
    }

    .event-type {
        color: var(--text-primary);
        font-weight: 600;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .badge {
        padding: 2px 6px;
        border-radius: var(--radius-md);
        font-size: 0.7rem;
        background: var(--accent-tint);
        color: var(--accent);
        box-shadow: inset 0 0 0 1px var(--accent-tint);
    }

    .cell-badge {
        background: var(--accent-tint);
        color: var(--accent);
    }

    .event-payload {
        color: var(--text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
</style>
