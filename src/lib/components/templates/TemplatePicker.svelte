<script lang="ts">
    import { onMount } from 'svelte';
    import { templates, selectedTemplate } from '../../stores/templates';
    import type { SessionTemplate } from '../../types/domain';
    import { MagnifyingGlass, Hexagon, TestTube, Scales } from 'phosphor-svelte';
    import Skeleton from '../Skeleton.svelte';
    import SkelBar from '../SkelBar.svelte';

    let searchQuery = '';

    $: filteredTemplates = $templates.templates.filter(t => 
        t.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
        t.description.toLowerCase().includes(searchQuery.toLowerCase())
    );

    onMount(() => {
        templates.fetchTemplates();
    });

    function selectTemplate(template: SessionTemplate) {
        selectedTemplate.set(template);
    }
</script>

{#snippet templateGridSkeleton()}
    <div class="templates-grid lattice-scroll-chrome">
        {#each [0, 1, 2] as _}
            <div class="template-skeleton-card lattice-forced-colors-boundary">
                <SkelBar width="44px" height="44px" radius="md" />
                <div class="template-skeleton-info">
                    <SkelBar width="62%" height="1rem" />
                    <div class="template-skeleton-description">
                        <SkelBar width="100%" height="0.65rem" />
                        <SkelBar width="78%" height="0.65rem" />
                    </div>
                    <div class="template-skeleton-meta">
                        <SkelBar width="3.5rem" height="0.55rem" />
                        <SkelBar width="4.5rem" height="0.55rem" />
                    </div>
                </div>
            </div>
        {/each}
    </div>
{/snippet}

<div class="template-picker">
    <div class="picker-header">
        <div class="search-box">
            <MagnifyingGlass size={14} weight="light" class="search-icon" />
            <input
                class="lattice-input"
                type="text" 
                placeholder="Search templates..." 
                bind:value={searchQuery}
            />
        </div>
        <button type="button" class="lattice-btn lattice-btn--secondary lattice-btn--dashed" on:click={() => {/* Open editor for new */}}>
            + New Template
        </button>
    </div>

    <Skeleton loading={$templates.loading} skeleton={templateGridSkeleton}>
        {#if filteredTemplates.length === 0}
            <div class="empty-state">No templates found.</div>
        {:else}
            <div class="templates-grid lattice-scroll-chrome">
                {#each filteredTemplates as template (template.id)}
                    <button
                        type="button"
                        class="lattice-btn lattice-btn--card"
                        aria-pressed={$selectedTemplate?.id === template.id}
                        on:click={() => selectTemplate(template)}
                        title={template.description}
                    >
                        <div
                            class="card-icon"
                            class:builtin={template.is_builtin}
                            class:lattice-forced-colors-boundary={template.is_builtin}
                        >
                            {#if template.mode === 'hive'}
                                <Hexagon size={24} weight="light" />
                            {:else if template.mode === 'debate'}
                                <Scales size={24} weight="light" />
                            {:else}
                                <TestTube size={24} weight="light" />
                            {/if}
                        </div>
                        <div class="card-info">
                            <div class="name-row">
                                <span class="name">{template.name}</span>
                                {#if template.is_builtin}
                                    <span class="badge">Built-in</span>
                                {/if}
                            </div>
                            <div class="description">{template.description}</div>
                            <div class="meta">
                                <span class="mode-tag">{template.mode}</span>
                                <span class="cells-tag">{template.cells.length} cells</span>
                            </div>
                        </div>
                    </button>
                {/each}
            </div>
        {/if}
    </Skeleton>
</div>

<style>
    .template-picker {
        display: flex;
        flex-direction: column;
        gap: 16px;
    }

    .picker-header {
        display: flex;
        justify-content: space-between;
        gap: 12px;
    }

    .search-box {
        flex: 1;
        position: relative;
        display: flex;
        align-items: center;
    }

    .search-box :global(.search-icon) {
        position: absolute;
        left: 12px;
        color: var(--text-secondary);
        opacity: 0.7;
        pointer-events: none;
    }

    .search-box input {
        width: 100%;
        padding: 8px 12px 8px 36px;
    }

    .templates-grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
        gap: 12px;
        max-height: 400px;
        overflow-y: auto;
        padding-right: 4px;
    }

    .template-skeleton-card {
        padding: 12px;
        display: flex;
        gap: 12px;
        box-shadow: inset 0 0 0 1px var(--border-structural);
        border-radius: var(--radius-md);
    }

    .template-skeleton-info,
    .template-skeleton-description {
        display: flex;
        flex: 1;
        flex-direction: column;
        gap: 4px;
        min-width: 0;
    }

    .template-skeleton-meta {
        display: flex;
        gap: 8px;
        margin-top: 4px;
    }

    .card-icon {
        font-size: 24px;
        width: 44px;
        height: 44px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: color-mix(in srgb, var(--bg-void) 80%, transparent);
        border-radius: var(--radius-sm);
    }

    .card-icon.builtin {
        box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent-cyan) 30%, transparent);
    }

    .card-info {
        flex: 1;
        overflow: hidden;
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .name-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
    }

    .name {
        font-weight: 600;
        color: var(--text-primary);
        font-size: 14px;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .badge {
        font-size: 9px;
        text-transform: uppercase;
        background: color-mix(in srgb, var(--accent-cyan) 20%, transparent);
        color: var(--accent-cyan);
        padding: 1px 4px;
        border-radius: var(--radius-sm);
        font-weight: 700;
    }

    .description {
        font-size: 11px;
        color: var(--text-secondary);
        display: -webkit-box;
        -webkit-line-clamp: 2;
        line-clamp: 2;
        -webkit-box-orient: vertical;
        overflow: hidden;
        line-height: 1.4;
    }

    .meta {
        display: flex;
        gap: 8px;
        margin-top: 4px;
    }

    .mode-tag, .cells-tag {
        font-size: 9px;
        text-transform: uppercase;
        color: var(--text-disabled);
        font-weight: 700;
        font-family: var(--font-mono);
    }

    .empty-state {
        padding: 40px;
        text-align: center;
        color: var(--text-secondary);
        font-style: italic;
    }
</style>
