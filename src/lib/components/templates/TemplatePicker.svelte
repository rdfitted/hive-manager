<script lang="ts">
    import { onMount } from 'svelte';
    import { templates, selectedTemplate } from '../../stores/templates';
    import type { SessionTemplate } from '../../types/domain';

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

<div class="template-picker">
    <div class="picker-header">
        <div class="search-box">
            <span class="search-icon">🔍</span>
            <input 
                type="text" 
                placeholder="Search templates..." 
                bind:value={searchQuery}
            />
        </div>
        <button class="new-btn" on:click={() => {/* Open editor for new */}}>
            + New Template
        </button>
    </div>

    {#if $templates.loading}
        <div class="loading-state">Loading templates...</div>
    {:else if filteredTemplates.length === 0}
        <div class="empty-state">No templates found.</div>
    {:else}
        <div class="templates-grid">
            {#each filteredTemplates as template (template.id)}
                <button 
                    class="template-card" 
                    on:click={() => selectTemplate(template)}
                    title={template.description}
                >
                    <div class="card-icon" class:builtin={template.is_builtin}>
                        {template.mode === 'hive' ? '🐝' : '🧪'}
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

    .search-icon {
        position: absolute;
        left: 12px;
        font-size: 14px;
        opacity: 0.5;
    }

    .search-box input {
        width: 100%;
        padding: 8px 12px 8px 36px;
        background: rgba(0, 0, 0, 0.3);
        border: 1px solid rgba(255, 255, 255, 0.1);
        border-radius: var(--radius-sm);
        color: var(--text-primary);
        font-size: 14px;
    }

    .new-btn {
        padding: 8px 16px;
        background: transparent;
        border: 1px dashed rgba(255, 255, 255, 0.2);
        border-radius: var(--radius-sm);
        color: var(--text-primary);
        font-size: 13px;
        cursor: pointer;
        white-space: nowrap;
    }

    .new-btn:hover {
        border-color: var(--accent-cyan);
        color: var(--accent-cyan);
    }

    .templates-grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
        gap: 12px;
        max-height: 400px;
        overflow-y: auto;
        padding-right: 4px;
    }

    .template-card {
        background: rgba(255, 255, 255, 0.03);
        border: 1px solid rgba(255, 255, 255, 0.08);
        border-radius: var(--radius-sm);
        padding: 12px;
        display: flex;
        gap: 12px;
        text-align: left;
        cursor: pointer;
        transition: all 0.2s;
    }

    .template-card:hover {
        background: rgba(255, 255, 255, 0.06);
        border-color: rgba(255, 255, 255, 0.15);
        transform: translateY(-2px);
    }

    .card-icon {
        font-size: 24px;
        width: 44px;
        height: 44px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: rgba(0, 0, 0, 0.2);
        border-radius: var(--radius-sm);
    }

    .card-icon.builtin {
        border: 1px solid rgba(139, 92, 246, 0.3);
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
        background: rgba(139, 92, 246, 0.2);
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

    .loading-state, .empty-state {
        padding: 40px;
        text-align: center;
        color: var(--text-secondary);
        font-style: italic;
    }
</style>
