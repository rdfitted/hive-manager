<script lang="ts">
    import { cells } from '$lib/stores/cells';
    import ArtifactSummary from './ArtifactSummary.svelte';
    import type { Cell } from '$lib/types/domain';

    $: allCells = Object.values($cells.cells);
    $: cellsWithArtifacts = allCells.filter(c => c.artifacts);

    let selectedCellIds: string[] = [];

    function toggleCellSelection(id: string) {
        if (selectedCellIds.includes(id)) {
            selectedCellIds = selectedCellIds.filter(i => i !== id);
        } else {
            selectedCellIds = [...selectedCellIds, id];
        }
    }

    $: displayedCells = selectedCellIds.length > 0 
        ? cellsWithArtifacts.filter(c => selectedCellIds.includes(c.id))
        : cellsWithArtifacts;
</script>

<div class="artifact-browser">
    <div class="browser-header">
        <h3>Artifact Browser</h3>
        <div class="cell-filters">
            <span class="label">Filter Cells:</span>
            {#each cellsWithArtifacts as cell}
                <button 
                    class="cell-chip {selectedCellIds.includes(cell.id) ? 'active' : ''}"
                    on:click={() => toggleCellSelection(cell.id)}
                >
                    {cell.id.substring(0, 8)}
                </button>
            {/each}
        </div>
    </div>

    <div class="artifacts-grid" class:comparison={selectedCellIds.length > 1}>
        {#if displayedCells.length === 0}
            <div class="empty-state">No artifacts available for the selected cells.</div>
        {:else}
            {#each displayedCells as cell}
                <div class="cell-column">
                    <div class="column-header">
                        Cell: {cell.id.substring(0, 8)}
                    </div>
                    <div class="column-content">
                        {#if cell.artifacts}
                            <ArtifactSummary artifact={cell.artifacts} />
                        {:else}
                            <div class="no-artifact">No artifact bundle yet.</div>
                        {/if}
                    </div>
                </div>
            {/each}
        {/if}
    </div>
</div>

<style>
    .artifact-browser {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--color-bg);
        overflow: hidden;
        font-family: 'JetBrains Mono', monospace;
    }

    .browser-header {
        padding: 16px;
        background: var(--color-surface);
        border-bottom: 1px solid var(--color-border);
        display: flex;
        justify-content: space-between;
        align-items: center;
    }

    .browser-header h3 {
        margin: 0;
        font-size: 1.1rem;
        color: var(--color-accent);
    }

    .cell-filters {
        display: flex;
        align-items: center;
        gap: 8px;
    }

    .label {
        font-size: 0.7rem;
        color: var(--color-text-muted);
        text-transform: uppercase;
        font-weight: bold;
    }

    .cell-chip {
        padding: 2px 8px;
        background: var(--color-bg);
        border: 1px solid var(--color-border);
        border-radius: 4px;
        color: var(--color-text-muted);
        font-size: 0.75rem;
        cursor: pointer;
        transition: all 0.2s;
    }

    .cell-chip.active {
        background: var(--color-accent);
        color: var(--color-bg);
        border-color: var(--color-accent);
        font-weight: bold;
    }

    .artifacts-grid {
        flex: 1;
        display: flex;
        gap: 16px;
        padding: 16px;
        overflow-x: auto;
        overflow-y: hidden;
    }

    .artifacts-grid.comparison {
        align-items: flex-start;
    }

    .cell-column {
        min-width: 350px;
        max-width: 500px;
        display: flex;
        flex-direction: column;
        background: var(--color-surface);
        border: 1px solid var(--color-border);
        border-radius: 8px;
        height: 100%;
    }

    .column-header {
        padding: 10px 16px;
        background: var(--color-surface-hover);
        border-bottom: 1px solid var(--color-border);
        font-weight: bold;
        color: var(--color-text);
        font-size: 0.9rem;
    }

    .column-content {
        padding: 16px;
        overflow-y: auto;
        flex: 1;
    }

    .empty-state, .no-artifact {
        padding: 40px;
        color: var(--color-text-muted);
        text-align: center;
        font-size: 0.9rem;
    }
</style>
