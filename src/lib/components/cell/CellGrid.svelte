<script lang="ts">
    import { cells } from '../../stores/cells';
    import { ui } from '../../stores/ui';
    import CellCard from './CellCard.svelte';

    $: cellList = Object.values($cells.cells).sort((a, b) => a.name.localeCompare(b.name));
    $: isCollapsed = $ui.cellGridCollapsed;

    function toggleGridCollapse() {
        ui.setCellGridCollapsed(!isCollapsed);
    }
</script>

<div class="cell-grid-container" class:collapsed={isCollapsed}>
    <div class="grid-header">
        <span class="title">Active Cells</span>
        <button class="collapse-btn" on:click={toggleGridCollapse}>
            {isCollapsed ? 'Expand' : 'Collapse'}
        </button>
    </div>
    
    <div class="grid" class:is-collapsed={isCollapsed}>
        {#each cellList as cell (cell.id)}
            <CellCard {cell} />
        {/each}
        {#if cellList.length === 0}
            <div class="empty">No cells active in this session.</div>
        {/if}
    </div>
</div>

<style>
    .cell-grid-container {
        display: flex;
        flex-direction: column;
        gap: 12px;
        padding: 16px;
        height: 100%;
        overflow: hidden;
    }

    .grid-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
    }

    .title {
        font-size: 11px;
        text-transform: uppercase;
        letter-spacing: 0.1em;
        color: #666;
        font-weight: 700;
    }

    .collapse-btn {
        background: transparent;
        border: 1px solid rgba(255, 255, 255, 0.1);
        color: #888;
        font-size: 10px;
        padding: 2px 8px;
        border-radius: 4px;
        cursor: pointer;
    }

    .collapse-btn:hover {
        background: rgba(255, 255, 255, 0.05);
        color: #fff;
    }

    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
        gap: 16px;
        overflow-y: auto;
        padding-right: 4px;
    }

    .grid.is-collapsed {
        grid-template-columns: 1fr;
        gap: 4px;
    }

    .empty {
        grid-column: 1 / -1;
        text-align: center;
        padding: 40px;
        color: #555;
        font-style: italic;
    }
</style>
