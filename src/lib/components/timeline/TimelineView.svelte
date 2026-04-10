<script lang="ts">
    import { filteredEvents, filters } from '$lib/stores/filters';
    import { events } from '$lib/stores/events';
    import { activeSession } from '$lib/stores/sessions';
    import EventListItem from './EventListItem.svelte';
    import EventFilterPanel from './EventFilterPanel.svelte';
    import EventDetailsModal from './EventDetailsModal.svelte';
    import type { Event as SessionEvent } from '$lib/types/domain';

    let selectedEvent: SessionEvent | null = null;

    $: if (selectedEvent && !$filteredEvents.some((event) => event.id === selectedEvent?.id)) {
        selectedEvent = null;
    }

    async function handleFetchHistory() {
        if ($activeSession) {
            await events.fetchEvents($activeSession.id);
        }
    }
</script>

<div class="timeline-container">
    <EventFilterPanel />

    <div class="event-list-header">
        <div class="header-item">Time</div>
        <div class="header-item">Type</div>
        <div class="header-item">Source</div>
        <div class="header-item">Payload</div>
    </div>

    <div class="event-list">
        {#if $filteredEvents.length === 0}
            <div class="empty-state">
                <p>No events found matching filters.</p>
                {#if $events.events.length === 0}
                    <button class="fetch-btn" on:click={handleFetchHistory}>
                        Load Historical Events
                    </button>
                {/if}
            </div>
        {:else}
            {#each $filteredEvents as event (event.id)}
                <EventListItem {event} on:select={(e) => selectedEvent = e.detail} />
            {/each}
        {/if}
    </div>
</div>

{#if selectedEvent}
    <EventDetailsModal event={selectedEvent} on:close={() => selectedEvent = null} />
{/if}

<style>
    .timeline-container {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--color-bg);
        overflow: hidden;
    }

    .event-list-header {
        display: grid;
        grid-template-columns: 4px 80px 140px 120px 1fr;
        gap: 12px;
        padding: 8px 12px;
        background: var(--color-surface-hover);
        border-bottom: 1px solid var(--color-border);
        font-family: var(--font-mono);
        font-size: 0.7rem;
        font-weight: bold;
        color: var(--color-text-muted);
        text-transform: uppercase;
    }

    .header-item {
        padding-left: 0;
    }

    .event-list {
        flex: 1;
        overflow-y: auto;
    }

    .empty-state {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        padding: 40px;
        color: var(--color-text-muted);
        text-align: center;
    }

    .fetch-btn {
        margin-top: 12px;
        padding: 8px 16px;
        background: var(--color-accent);
        color: var(--color-bg);
        border: none;
        border-radius: var(--radius-sm);
        cursor: pointer;
        font-weight: 600;
        transition: opacity 0.2s;
    }

    .fetch-btn:hover {
        opacity: 0.9;
    }
</style>
