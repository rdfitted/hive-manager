<script lang="ts">
    import { cells } from '../../stores/cells';
    import ArtifactSummary from '../artifacts/ArtifactSummary.svelte';
    export let sessionId: string;

    $: sessionCells = Object.values($cells.cells).filter(c => c.session_id === sessionId);
    $: candidates = sessionCells.filter(c => c.cell_type !== 'resolver');

    function getStatusIcon(status: string) {
        return {
            'queued': '⏳',
            'preparing': '🛠️',
            'launching': '🚀',
            'running': '⚡',
            'summarizing': '📝',
            'completed': '✅',
            'waiting_input': '❓',
            'failed': '❌',
            'killed': '💀'
        }[status] || '❓';
    }
</script>

<div class="fusion-comparison-view">
    <div class="grid" style="grid-template-columns: repeat({Math.max(1, candidates.length)}, 1fr);">
        {#each candidates as cell (cell.id)}
            <div class="candidate-card" class:completed={cell.status === 'completed'} class:failed={cell.status === 'failed'}>
                <div class="card-header">
                    <div class="status-row">
                        <span class="status-badge" title={cell.status}>{getStatusIcon(cell.status)} {cell.status}</span>
                        <span class="type-tag">{cell.cell_type}</span>
                    </div>
                    <h3 class="name">{cell.name}</h3>
                    <div class="branch-info">
                        <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/></svg>
                        {cell.workspace.branch_name}
                    </div>
                </div>

                <div class="card-content">
                    {#if cell.artifacts}
                        <ArtifactSummary artifact={cell.artifacts} />
                    {:else if cell.status === 'running' || cell.status === 'summarizing'}
                        <div class="loading-state">
                            <div class="spinner"></div>
                            <span>Collecting artifacts...</span>
                        </div>
                    {:else}
                        <div class="empty-state">
                            No artifacts available.
                        </div>
                    {/if}
                </div>
            </div>
        {/each}
    </div>
</div>

<style>
    .fusion-comparison-view {
        width: 100%;
        height: 100%;
        overflow-x: auto;
        padding: 16px;
        background: color-mix(in srgb, var(--bg-void) 45%, var(--bg-surface));
    }

    .grid {
        display: grid;
        gap: 16px;
        min-width: min-content;
    }

    .candidate-card {
        background: color-mix(in srgb, var(--text-primary) 3%, var(--bg-surface));
        border: 1px solid color-mix(in srgb, var(--text-primary) 8%, transparent);
        border-radius: var(--radius-sm);
        display: flex;
        flex-direction: column;
        min-width: 320px;
        max-width: 500px;
        transition: all 0.2s ease;
    }

    .candidate-card.completed {
        border-color: color-mix(in srgb, var(--status-success) 20%, transparent);
        background: color-mix(in srgb, var(--status-success) 4%, var(--bg-surface));
    }

    .candidate-card.failed {
        border-color: color-mix(in srgb, var(--status-error) 20%, transparent);
        background: color-mix(in srgb, var(--status-error) 4%, var(--bg-surface));
    }

    .card-header {
        padding: 16px;
        border-bottom: 1px solid color-mix(in srgb, var(--text-primary) 5%, transparent);
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .status-row {
        display: flex;
        justify-content: space-between;
        align-items: center;
    }

    .status-badge {
        font-size: 10px;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        font-weight: 700;
        color: var(--text-secondary);
        background: color-mix(in srgb, var(--bg-void) 60%, var(--bg-surface));
        padding: 2px 8px;
        border-radius: var(--radius-sm);
    }

    .type-tag {
        font-size: 9px;
        text-transform: uppercase;
        letter-spacing: 0.1em;
        color: var(--text-disabled);
        font-weight: 800;
    }

    .name {
        margin: 0;
        font-size: 18px;
        font-weight: 600;
        color: var(--text-primary);
    }

    .branch-info {
        display: flex;
        align-items: center;
        gap: 6px;
        font-size: 11px;
        color: var(--text-secondary);
        font-family: var(--font-mono);
    }

    .card-content {
        padding: 16px;
        flex: 1;
        display: flex;
        flex-direction: column;
        gap: 16px;
    }

    .loading-state, .empty-state {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        padding: 40px 20px;
        color: var(--text-disabled);
        font-size: 13px;
        text-align: center;
        gap: 12px;
    }

    .spinner {
        width: 24px;
        height: 24px;
        border: 2px solid color-mix(in srgb, var(--text-primary) 10%, transparent);
        border-top-color: var(--accent-cyan);
        border-radius: 50%;
        animation: spin 1s linear infinite;
    }

    @keyframes spin {
        to { transform: rotate(360deg); }
    }
</style>
