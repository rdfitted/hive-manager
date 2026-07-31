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

<div class="filter-panel lattice-panel lattice-scroll-chrome">
    <div class="filter-row">
        <input 
            type="text" 
            placeholder="Search payload..." 
            value={$filters.searchText}
            on:input={(e) => filters.setSearchText(e.currentTarget.value)}
            class="lattice-input search-input"
        />

        <div class="filter-group">
            <span class="label">Severity:</span>
            {#each SEVERITIES as s}
                <button
                    type="button"
                    class="lattice-btn lattice-btn--chip {$filters.severities.includes(s) ? 'lattice-btn--selected' : ''}"
                    aria-pressed={$filters.severities.includes(s)}
                    on:click={() => filters.toggleSeverity(s)}
                >
                    {s}
                </button>
            {/each}
        </div>

        <button type="button" class="lattice-btn lattice-btn--ghost lattice-btn--compact" on:click={() => filters.reset()}>Clear</button>
    </div>

    <div class="filter-row wrap">
        <div class="filter-group">
            <span class="label">Type:</span>
            <div class="chip-container">
                {#each EVENT_TYPES as t}
                    <button
                        type="button"
                        class="lattice-btn lattice-btn--chip {$filters.types.includes(t) ? 'lattice-btn--selected' : ''}"
                        aria-pressed={$filters.types.includes(t)}
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
                class="lattice-input filter-select"
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
                class="lattice-input filter-select"
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
    }

    .chip-container {
        display: flex;
        flex-wrap: wrap;
        gap: 4px;
    }

</style>
