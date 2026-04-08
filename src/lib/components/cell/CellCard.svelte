<script lang="ts">
    import type { Cell } from '../../types/domain';
    import WorkspaceBadge from './WorkspaceBadge.svelte';
    import AgentList from '../agent/AgentList.svelte';
    import { ui } from '../../stores/ui';

    export let cell: Cell;

    $: isSelected = $ui.selectedCellId === cell.id;
    $: isCollapsed = $ui.cellGridCollapsed;

    function toggleSelection() {
        if (isSelected) {
            ui.setSelectedCell(null);
        } else {
            ui.setSelectedCell(cell.id);
        }
    }

    $: statusIcon = {
        'queued': '⏳',
        'preparing': '🛠️',
        'launching': '🚀',
        'running': '⚡',
        'summarizing': '📝',
        'completed': '✅',
        'waiting_input': '❓',
        'failed': '❌',
        'killed': '💀'
    }[cell.status] || '❓';
</script>

<div 
    class="cell-card" 
    class:selected={isSelected} 
    class:collapsed={isCollapsed}
    on:click={toggleSelection}
>
    <div class="header">
        <div class="status-icon" title={cell.status}>{statusIcon}</div>
        <div class="name-box">
            <span class="type-tag">{cell.cell_type}</span>
            <span class="name">{cell.name}</span>
        </div>
        {#if isCollapsed}
            <div class="collapsed-info">
                <span class="agent-count">👥 {cell.agents.length}</span>
                <span class="objective-preview">{cell.objective}</span>
            </div>
        {/if}
    </div>

    {#if !isCollapsed}
        <div class="content">
            <div class="objective">{cell.objective}</div>
            
            <div class="meta">
                <WorkspaceBadge workspace={cell.workspace} />
            </div>

            <div class="section">
                <div class="section-label">Agents</div>
                <AgentList agentIds={cell.agents} />
            </div>

            {#if cell.artifacts}
                <div class="section">
                    <div class="section-label">Changes</div>
                    <div class="changed-files">
                        {#each cell.artifacts.changed_files.slice(0, 5) as file}
                            <div class="file-item">
                                <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>
                                {file}
                            </div>
                        {/each}
                        {#if cell.artifacts.changed_files.length > 5}
                            <div class="more-files">+{cell.artifacts.changed_files.length - 5} more</div>
                        {/if}
                    </div>
                </div>
            {/if}
        </div>
    {/if}
</div>

<style>
    .cell-card {
        background: rgba(255, 255, 255, 0.04);
        border: 1px solid rgba(255, 255, 255, 0.1);
        border-radius: 8px;
        transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
        cursor: pointer;
        overflow: hidden;
        display: flex;
        flex-direction: column;
    }

    .cell-card:hover {
        background: rgba(255, 255, 255, 0.07);
        border-color: rgba(255, 255, 255, 0.2);
    }

    .cell-card.selected {
        background: rgba(59, 130, 246, 0.08);
        border-color: rgba(59, 130, 246, 0.5);
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
    }

    .header {
        padding: 12px;
        display: flex;
        align-items: center;
        gap: 12px;
        border-bottom: 1px solid transparent;
    }

    .cell-card:not(.collapsed) .header {
        border-bottom: 1px solid rgba(255, 255, 255, 0.05);
    }

    .status-icon {
        font-size: 18px;
        width: 32px;
        height: 32px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: rgba(0, 0, 0, 0.3);
        border-radius: 8px;
    }

    .name-box {
        display: flex;
        flex-direction: column;
    }

    .type-tag {
        font-size: 9px;
        text-transform: uppercase;
        letter-spacing: 0.1em;
        color: #888;
        font-weight: 700;
    }

    .name {
        font-weight: 600;
        font-size: 14px;
        color: #fff;
    }

    .collapsed-info {
        display: flex;
        align-items: center;
        gap: 16px;
        flex: 1;
        margin-left: 12px;
        overflow: hidden;
    }

    .agent-count {
        font-size: 11px;
        color: #888;
        white-space: nowrap;
    }

    .objective-preview {
        font-size: 12px;
        color: #666;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .content {
        padding: 12px;
        display: flex;
        flex-direction: column;
        gap: 16px;
    }

    .objective {
        font-size: 13px;
        color: #bbb;
        line-height: 1.4;
    }

    .section-label {
        font-size: 10px;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        color: #555;
        font-weight: 700;
        margin-bottom: 8px;
    }

    .changed-files {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .file-item {
        display: flex;
        align-items: center;
        gap: 6px;
        font-size: 11px;
        color: #888;
        font-family: var(--font-mono);
    }

    .more-files {
        font-size: 10px;
        color: #555;
        padding-left: 18px;
    }

    /* Collapsed state adjustments */
    .cell-card.collapsed {
        border-radius: 4px;
    }

    .cell-card.collapsed .header {
        padding: 6px 12px;
    }

    .cell-card.collapsed .status-icon {
        font-size: 14px;
        width: 24px;
        height: 24px;
    }

    .cell-card.collapsed .name {
        font-size: 12px;
    }
</style>
