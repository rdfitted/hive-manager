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
            case 'error': return 'var(--color-error)';
            case 'warning': return 'var(--color-warning)';
            default: return 'var(--color-accent)';
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

<!-- svelte-ignore a11y-click-events-have-key-events -->
<!-- svelte-ignore a11y-no-static-element-interactions -->
<div class="event-item" on:click={() => dispatch('select', event)}>
    <div class="severity-indicator" style="background-color: {getSeverityColor(event.severity)}"></div>
    
    <div class="event-time">{formatTime(event.timestamp)}</div>
    
    <div class="event-type" title={event.event_type}>
        {getEventTypeLabel(event.event_type)}
    </div>

    <div class="event-source">
        {#if event.cell_id}
            <span class="badge cell-badge">{event.cell_id.substring(0, 8)}</span>
        {/if}
        {#if event.agent_id}
            <span class="badge agent-badge">{event.agent_id.substring(0, 8)}</span>
        {/if}
    </div>

    <div class="event-payload">
        {getPayloadExcerpt(event.payload)}
    </div>
</div>

<style>
    .event-item {
        display: grid;
        grid-template-columns: 4px 80px 140px 120px 1fr;
        gap: 12px;
        align-items: center;
        padding: 8px 12px;
        background: var(--color-surface);
        border-bottom: 1px solid var(--color-border);
        cursor: pointer;
        font-family: var(--font-mono);
        font-size: 0.8rem;
        transition: background 0.2s;
    }

    .event-item:hover {
        background: var(--color-surface-hover);
    }

    .severity-indicator {
        height: 100%;
        width: 4px;
        border-radius: 2px;
    }

    .event-time {
        color: var(--color-text-muted);
    }

    .event-type {
        color: var(--color-text);
        font-weight: 600;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .badge {
        padding: 2px 6px;
        border-radius: var(--radius-sm);
        font-size: 0.7rem;
        background: var(--color-accent-dim);
        color: var(--color-accent);
        border: 1px solid var(--color-accent-dim);
    }

    .cell-badge {
        background: rgba(139, 92, 246, 0.1);
        color: var(--accent-cyan);
    }

    .event-payload {
        color: var(--color-text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
</style>
