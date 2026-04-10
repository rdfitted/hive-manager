<script lang="ts">
    import { filters } from '$lib/stores/filters';
    import { activeSession } from '$lib/stores/sessions';
    import { cells as cellsStore } from '$lib/stores/cells';
    import type { EventType, Severity } from '$lib/types/domain';

    const EVENT_TYPES: EventType[] = [
        'session_created',
        'session_status_changed',
        'cell_created',
        'cell_status_changed',
        'workspace_created',
        'agent_launched',
        'agent_completed',
        'agent_waiting_input',
        'agent_failed',
        'artifact_updated',
        'resolver_selected_candidate',
    ];

    const SEVERITIES: Severity[] = ['info', 'warning', 'error'];

    $: cells = Object.values($cellsStore.cells);
    $: agents = $activeSession?.agents ?? [];
</script>

<div class="filter-panel">
    <div class="filter-row">
        <input 
            type="text" 
            placeholder="Search payload..." 
            value={$filters.searchText}
            on:input={(e) => filters.setSearchText(e.currentTarget.value)}
            class="search-input"
        />

        <div class="filter-group">
            <span class="label">Severity:</span>
            {#each SEVERITIES as s}
                <button 
                    class="filter-chip {s} {$filters.severities.includes(s) ? 'active' : ''}"
                    on:click={() => filters.toggleSeverity(s)}
                >
                    {s}
                </button>
            {/each}
        </div>

        <button class="clear-btn" on:click={() => filters.reset()}>Clear</button>
    </div>

    <div class="filter-row wrap">
        <div class="filter-group">
            <span class="label">Type:</span>
            <div class="chip-container">
                {#each EVENT_TYPES as t}
                    <button 
                        class="filter-chip type {$filters.types.includes(t) ? 'active' : ''}"
                        on:click={() => filters.toggleType(t)}
                    >
                        {t.replace(/_/g, ' ')}
                    </button>
                {/each}
            </div>
        </div>
    </div>

    <div class="filter-row">
        <div class="filter-group">
            <span class="label">Cell:</span>
            <select 
                value={$filters.cellId || ''} 
                on:change={(e) => filters.setCellId(e.currentTarget.value || null)}
                class="filter-select"
            >
                <option value="">All Cells</option>
                {#each cells as cell}
                    <option value={cell.id}>{cell.id.substring(0, 8)}</option>
                {/each}
            </select>
        </div>

        <div class="filter-group">
            <span class="label">Agent:</span>
            <select 
                value={$filters.agentId || ''} 
                on:change={(e) => filters.setAgentId(e.currentTarget.value || null)}
                class="filter-select"
            >
                <option value="">All Agents</option>
                {#each agents as agent}
                    <option value={agent.id}>{agent.config?.label || agent.id.substring(0, 8)}</option>
                {/each}
            </select>
        </div>
    </div>
</div>

<style>
    .filter-panel {
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 12px;
        background: var(--color-surface);
        border-bottom: 1px solid var(--color-border);
        font-family: var(--font-mono);
        font-size: 0.8rem;
    }

    .filter-row {
        display: flex;
        align-items: center;
        gap: 16px;
    }

    .filter-row.wrap {
        flex-wrap: wrap;
    }

    .filter-group {
        display: flex;
        align-items: center;
        gap: 8px;
    }

    .label {
        color: var(--color-text-muted);
        font-weight: bold;
        font-size: 0.7rem;
        text-transform: uppercase;
    }

    .search-input {
        flex: 1;
        padding: 4px 8px;
        background: var(--color-bg);
        border: 1px solid var(--color-border);
        border-radius: var(--radius-sm);
        color: var(--color-text);
        font-family: inherit;
        font-size: inherit;
    }

    .search-input:focus {
        border-color: var(--color-accent);
        outline: none;
    }

    .filter-chip {
        padding: 2px 8px;
        background: var(--color-bg);
        border: 1px solid var(--color-border);
        border-radius: var(--radius-sm);
        color: var(--color-text-muted);
        cursor: pointer;
        font-size: 0.7rem;
        transition: all 0.2s;
    }

    .filter-chip.active {
        color: var(--color-bg);
        font-weight: 600;
    }

    .filter-chip.info.active { background: var(--color-accent); border-color: var(--color-accent); }
    .filter-chip.warning.active { background: var(--color-warning); border-color: var(--color-warning); }
    .filter-chip.error.active { background: var(--color-error); border-color: var(--color-error); }
    .filter-chip.type.active { background: var(--color-text); border-color: var(--color-text); }

    .chip-container {
        display: flex;
        flex-wrap: wrap;
        gap: 4px;
    }

    .filter-select {
        padding: 4px;
        background: var(--color-bg);
        border: 1px solid var(--color-border);
        border-radius: var(--radius-sm);
        color: var(--color-text);
        font-family: inherit;
        font-size: 0.7rem;
    }

    .clear-btn {
        padding: 4px 12px;
        background: transparent;
        border: 1px solid var(--color-border);
        border-radius: var(--radius-sm);
        color: var(--color-text-muted);
        cursor: pointer;
        font-size: 0.7rem;
    }

    .clear-btn:hover {
        border-color: var(--color-error);
        color: var(--color-error);
    }
</style>
