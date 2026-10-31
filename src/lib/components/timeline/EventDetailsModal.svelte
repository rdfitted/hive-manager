<script lang="ts">
    import { Dialog } from '../ui';
    import type { Event } from '$lib/types/domain';
    import { createEventDispatcher } from 'svelte';

    export let event: Event;

    const dispatch = createEventDispatcher();
    let copyFeedback = '';

    function formatJSON(payload: any) {
        return JSON.stringify(payload, null, 2);
    }

    async function copyToClipboard() {
        try {
            await navigator.clipboard.writeText(JSON.stringify(event, null, 2));
            copyFeedback = 'Copied to clipboard';
        } catch (error) {
            console.error('Failed to copy event JSON', error);
            copyFeedback = 'Clipboard write failed';
        }
    }

    function closeModal() {
        dispatch('close');
    }

</script>

<Dialog open={true} label="Event details" onOpenChange={(value) => { if (!value) closeModal(); }}>
    <div class="modal-content">
        <div class="modal-header lattice-forced-colors-boundary">
            <h3>Event Details</h3>
            <div class="header-actions">
                <button type="button" class="lattice-btn lattice-btn--secondary lattice-btn--compact" on:click={copyToClipboard}>Copy JSON</button>
                <button type="button" class="lattice-btn lattice-btn--ghost lattice-btn--icon" on:click={closeModal} aria-label="Close event details">&times;</button>
            </div>
        </div>

        <div class="modal-body lattice-scroll-content">
            {#if copyFeedback}
                <p class="copy-feedback">{copyFeedback}</p>
            {/if}
            <div class="info-grid">
                <span class="label">ID:</span> <span class="value">{event.id}</span>
                <span class="label">Type:</span> <span class="value">{event.event_type}</span>
                <span class="label">Time:</span> <span class="value">{event.timestamp}</span>
                <span class="label">Cell:</span> <span class="value">{event.cell_id || 'N/A'}</span>
                <span class="label">Agent:</span> <span class="value">{event.agent_id || 'N/A'}</span>
                <span class="label">Severity:</span> <span class="value {event.severity}">{event.severity}</span>
            </div>

            <div class="payload-section">
                <h4 class="lattice-forced-colors-boundary">Payload</h4>
                <pre class="lattice-scroll-content">{formatJSON(event.payload)}</pre>
            </div>
        </div>
    </div>
</Dialog>


<style>
    .modal-content {
        position: relative;
        width: min(800px, 90vw);
        max-width: 800px;
        max-height: 80vh;
        display: flex;
        flex-direction: column;
    }

    .modal-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        padding: 16px;
        box-shadow: var(--edge-seam);
    }

    .modal-header h3 {
        margin: 0;
        color: var(--color-accent);
    }

    .header-actions {
        display: flex;
        gap: 8px;
    }

    .modal-body {
        padding: 20px;
        overflow-y: auto;
    }

    .copy-feedback {
        margin: 0 0 16px 0;
        color: var(--color-text-muted);
        font-size: 0.85rem;
    }

    .info-grid {
        display: grid;
        grid-template-columns: 100px 1fr;
        gap: 8px;
        margin-bottom: 24px;
        font-size: 0.9rem;
    }

    .label {
        color: var(--color-text-muted);
        font-weight: bold;
    }

    .value.error { color: var(--color-error); }
    .value.warning { color: var(--color-warning); }
    .value.info { color: var(--color-accent); }

    .payload-section h4 {
        margin: 0 0 12px 0;
        color: var(--color-text-muted);
        box-shadow: var(--edge-seam);
        padding-bottom: 4px;
    }

    pre {
        font-family: var(--font-mono);
        margin: 0;
        padding: 12px;
        background: var(--bg-void);
        border-radius: var(--radius-md);
        color: var(--status-success);
        font-size: 0.85rem;
        overflow-x: auto;
    }
</style>
