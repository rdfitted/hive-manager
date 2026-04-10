<script lang="ts">
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

    function handleWindowKeydown(event: KeyboardEvent) {
        if (event.key === 'Escape') {
            closeModal();
        }
    }

    function handleBackdropKeydown(event: KeyboardEvent) {
        if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            closeModal();
        }
    }
</script>

<svelte:window on:keydown={handleWindowKeydown} />

<div class="modal-backdrop">
    <button
        type="button"
        class="modal-dismiss"
        aria-label="Close event details"
        on:click={closeModal}
        on:keydown={handleBackdropKeydown}
    ></button>
    <div
        class="modal-content"
        role="dialog"
        aria-modal="true"
        aria-label="Event details"
        tabindex="-1"
    >
        <div class="modal-header">
            <h3>Event Details</h3>
            <div class="header-actions">
                <button on:click={copyToClipboard}>Copy JSON</button>
                <button class="close-btn" on:click={closeModal} aria-label="Close event details">&times;</button>
            </div>
        </div>

        <div class="modal-body">
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
                <h4>Payload</h4>
                <pre>{formatJSON(event.payload)}</pre>
            </div>
        </div>
    </div>
</div>

<style>
    .modal-backdrop {
        position: fixed;
        top: 0;
        left: 0;
        width: 100vw;
        height: 100vh;
        background: rgba(0, 0, 0, 0.7);
        display: flex;
        align-items: center;
        justify-content: center;
        z-index: 1000;
        font-family: 'JetBrains Mono', monospace;
    }

    .modal-dismiss {
        position: absolute;
        inset: 0;
        border: 0;
        background: transparent;
        padding: 0;
        cursor: default;
    }

    .modal-content {
        position: relative;
        width: 80%;
        max-width: 800px;
        max-height: 80%;
        background: var(--color-bg);
        border: 1px solid var(--color-border);
        border-radius: 8px;
        display: flex;
        flex-direction: column;
        box-shadow: 0 10px 30px rgba(0, 0, 0, 0.5);
    }

    .modal-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        padding: 16px;
        border-bottom: 1px solid var(--color-border);
    }

    .modal-header h3 {
        margin: 0;
        color: var(--color-accent);
    }

    .header-actions {
        display: flex;
        gap: 8px;
    }

    .header-actions button {
        padding: 4px 12px;
        background: var(--color-surface);
        border: 1px solid var(--color-border);
        border-radius: 4px;
        color: var(--color-text);
        cursor: pointer;
    }

    .close-btn {
        font-size: 1.5rem;
        line-height: 1;
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
        border-bottom: 1px solid var(--color-border);
        padding-bottom: 4px;
    }

    pre {
        margin: 0;
        padding: 12px;
        background: #0f111a;
        border-radius: 4px;
        color: #9ece6a;
        font-size: 0.85rem;
        overflow-x: auto;
    }
</style>
