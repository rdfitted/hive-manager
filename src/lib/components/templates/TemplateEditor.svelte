<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import { templates } from '../../stores/templates';
    import type { SessionTemplate, CellTemplate } from '../../types/domain';
    import AgentConfigEditor from '../AgentConfigEditor.svelte';

    export let template: SessionTemplate | null = null;

    const dispatch = createEventDispatcher();

    let id = template?.is_builtin ? '' : (template?.id || '');
    let name = template?.name || '';
    let description = template?.description || '';
    let mode: 'hive' | 'fusion' = template?.mode || 'hive';
    let cells: CellTemplate[] = template?.cells ? JSON.parse(JSON.stringify(template.cells)) : [];
    let workspace_strategy: SessionTemplate['workspace_strategy'] = template?.workspace_strategy || 'shared_cell';
    let is_builtin = template?.is_builtin || false;
    let error = '';

    function addCell() {
        cells = [...cells, {
            role: 'general',
            cli: 'claude',
            prompt_template: 'general'
        }];
    }

    function removeCell(index: number) {
        cells = cells.filter((_, i) => i !== index);
    }

    async function handleSave() {
        if (cells.length === 0) {
            error = 'Add at least one cell before saving a template.';
            return;
        }

        error = '';
        const newTemplate: SessionTemplate = {
            id: id || crypto.randomUUID(),
            name,
            description,
            mode,
            cells,
            workspace_strategy,
            is_builtin: false // User saved templates are never builtin
        };

        try {
            await templates.saveTemplate(newTemplate);
            dispatch('save', newTemplate);
        } catch (err) {
            error = err instanceof Error ? err.message : 'Failed to save template.';
        }
    }

    function handleCancel() {
        dispatch('cancel');
    }
</script>

<div class="template-editor">
    <div class="header">
        <h3>{is_builtin ? 'Clone Template' : (template ? 'Edit Template' : 'New Template')}</h3>
        {#if is_builtin}
            <div class="info-badge">Built-in templates cannot be modified. Saving will create a new custom template.</div>
        {/if}
    </div>

    {#if error}
        <div class="error-banner" role="alert">{error}</div>
    {/if}

    <div class="form-section">
        <div class="form-group">
            <label for="template-name">Template Name</label>
            <input id="template-name" type="text" bind:value={name} placeholder="e.g. My Custom Hive" />
        </div>

        <div class="form-group">
            <label for="template-desc">Description</label>
            <textarea id="template-desc" bind:value={description} placeholder="What is this template for?" rows="2"></textarea>
        </div>

        <div class="form-row">
            <div class="form-group">
                <label for="template-mode">Session Mode</label>
                <select id="template-mode" bind:value={mode}>
                    <option value="hive">Hive</option>
                    <option value="fusion">Fusion</option>
                </select>
            </div>
            <div class="form-group">
                <label for="template-strategy">Workspace Strategy</label>
                <select id="template-strategy" bind:value={workspace_strategy}>
                    <option value="none">None</option>
                    <option value="shared_cell">Shared Cell</option>
                    <option value="isolated_cell">Isolated Cell</option>
                </select>
            </div>
        </div>
    </div>

    <div class="cells-section">
        <div class="section-header">
            <h4>Cells ({cells.length})</h4>
            <button class="add-btn" on:click={addCell}>+ Add Cell</button>
        </div>

        <div class="cells-list">
            {#each cells as cell, i}
                <div class="cell-editor-card">
                    <div class="card-header">
                        <span class="cell-num">Cell {i + 1}</span>
                        <button class="remove-btn" on:click={() => removeCell(i)}>Remove</button>
                    </div>
                    
                    <div class="form-row">
                        <div class="form-group">
                            <label>Role</label>
                            <input type="text" bind:value={cell.role} placeholder="e.g. backend" />
                        </div>
                        <div class="form-group">
                            <label>CLI</label>
                            <input type="text" bind:value={cell.cli} placeholder="e.g. claude" />
                        </div>
                    </div>

                    <div class="form-group">
                        <label>Model (optional)</label>
                        <input type="text" bind:value={cell.model} placeholder="e.g. opus" />
                    </div>

                    <div class="form-group">
                        <label>Prompt Template Key</label>
                        <input type="text" bind:value={cell.prompt_template} placeholder="e.g. backend" />
                    </div>
                </div>
            {/each}
        </div>
    </div>

    <div class="actions">
        <button class="cancel-btn" on:click={handleCancel}>Cancel</button>
        <button class="save-btn" on:click={handleSave} disabled={!name}>Save Template</button>
    </div>
</div>

<style>
    .template-editor {
        display: flex;
        flex-direction: column;
        gap: 20px;
        padding: 4px;
    }

    .header h3 {
        margin: 0;
        font-size: 16px;
        color: var(--text-primary);
    }

    .info-badge {
        margin-top: 8px;
        font-size: 11px;
        color: var(--accent-cyan);
        background: color-mix(in srgb, var(--accent-cyan) 10%, transparent);
        padding: 6px 10px;
        border-radius: var(--radius-sm);
        border: 1px solid color-mix(in srgb, var(--accent-cyan) 20%, transparent);
    }

    .error-banner {
        padding: 10px 12px;
        border-radius: var(--radius-sm);
        background: color-mix(in srgb, var(--status-error) 12%, transparent);
        border: 1px solid color-mix(in srgb, var(--status-error) 35%, transparent);
        color: var(--status-error);
        font-size: 12px;
    }

    .form-section {
        display: flex;
        flex-direction: column;
        gap: 12px;
    }

    .form-group {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .form-row {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 12px;
    }

    label {
        font-size: 12px;
        font-weight: 600;
        color: var(--text-secondary);
    }

    input, textarea, select {
        background: color-mix(in srgb, var(--bg-void) 70%, transparent);
        border: 1px solid color-mix(in srgb, var(--text-primary) 10%, transparent);
        border-radius: var(--radius-sm);
        padding: 8px 12px;
        color: var(--text-primary);
        font-size: 14px;
        font-family: inherit;
    }

    input:focus, textarea:focus, select:focus {
        outline: none;
        border-color: var(--accent-cyan);
    }

    .cells-section {
        display: flex;
        flex-direction: column;
        gap: 12px;
    }

    .section-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
    }

    .section-header h4 {
        margin: 0;
        font-size: 14px;
        color: var(--text-primary);
    }

    .add-btn {
        padding: 4px 12px;
        background: color-mix(in srgb, var(--accent-cyan) 10%, transparent);
        border: 1px solid color-mix(in srgb, var(--accent-cyan) 30%, transparent);
        border-radius: var(--radius-sm);
        color: var(--accent-cyan);
        font-size: 12px;
        cursor: pointer;
    }

    .cells-list {
        display: flex;
        flex-direction: column;
        gap: 12px;
        max-height: 300px;
        overflow-y: auto;
        padding-right: 4px;
    }

    .cell-editor-card {
        background: color-mix(in srgb, var(--text-primary) 3%, transparent);
        border: 1px solid color-mix(in srgb, var(--text-primary) 8%, transparent);
        border-radius: var(--radius-sm);
        padding: 12px;
        display: flex;
        flex-direction: column;
        gap: 12px;
    }

    .card-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
    }

    .cell-num {
        font-size: 11px;
        font-weight: 800;
        text-transform: uppercase;
        color: var(--text-disabled);
    }

    .remove-btn {
        font-size: 11px;
        color: var(--status-error);
        background: transparent;
        border: none;
        cursor: pointer;
        padding: 2px 6px;
    }

    .remove-btn:hover {
        background: color-mix(in srgb, var(--status-error) 10%, transparent);
        border-radius: var(--radius-sm);
    }

    .actions {
        display: flex;
        justify-content: flex-end;
        gap: 12px;
        margin-top: 12px;
        padding-top: 20px;
        border-top: 1px solid color-mix(in srgb, var(--text-primary) 10%, transparent);
    }

    .cancel-btn, .save-btn {
        padding: 8px 20px;
        border-radius: var(--radius-sm);
        font-size: 14px;
        font-weight: 600;
        cursor: pointer;
        border: none;
    }

    .cancel-btn {
        background: color-mix(in srgb, var(--text-primary) 5%, transparent);
        color: var(--text-primary);
    }

    .save-btn {
        background: var(--accent-cyan);
        color: var(--bg-void);
    }

    .save-btn:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }
</style>
