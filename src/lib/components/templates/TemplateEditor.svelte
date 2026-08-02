<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import { templates } from '../../stores/templates';
    import type { SessionTemplate, CellTemplate, SessionMode } from '../../types/domain';
    import AgentConfigEditor from '../AgentConfigEditor.svelte';
    import { routeFusionTemplateCells } from './templateLaunch';

    export let template: SessionTemplate | null = null;

    const dispatch = createEventDispatcher();

    let id = template?.is_builtin ? '' : (template?.id || '');
    let name = template?.name || '';
    let description = template?.description || '';
    let mode: SessionMode = template?.mode || 'hive';
    let cells: CellTemplate[] = template?.cells ? JSON.parse(JSON.stringify(template.cells)) : [];
    // `none` is reserved for the direct Research launch profile, which does
    // not use custom templates. Normalize historical template values here.
    let workspace_strategy: SessionTemplate['workspace_strategy'] =
        template?.workspace_strategy === 'isolated_cell' ? 'isolated_cell' : 'shared_cell';
    let is_builtin = template?.is_builtin || false;
    let error = '';

    $: if (mode !== 'hive') workspace_strategy = 'isolated_cell';

    function addCell() {
        cells = [...cells, {
            role: 'principal',
            cli: 'codex',
            model: 'gpt-5.6-sol',
            prompt_template: 'principal'
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

        if (mode === 'fusion' && routeFusionTemplateCells(cells).variants.length === 0) {
            error = 'Fusion templates need at least one candidate cell in addition to any judge or resolver.';
            return;
        }

        error = '';
        const newTemplate: SessionTemplate = {
            id: id || crypto.randomUUID(),
            name,
            description,
            mode,
            cells,
            workspace_strategy: mode === 'hive' ? workspace_strategy : 'isolated_cell',
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
            <div class="info-badge lattice-forced-colors-boundary">Built-in templates cannot be modified. Saving will create a new custom template.</div>
        {/if}
    </div>

    {#if error}
        <div class="error-banner lattice-forced-colors-boundary" role="alert">{error}</div>
    {/if}

    <div class="form-section">
        <div class="form-group">
            <label for="template-name">Template Name</label>
            <input class="lattice-input" id="template-name" type="text" bind:value={name} placeholder="e.g. My Custom Hive" />
        </div>

        <div class="form-group">
            <label for="template-desc">Description</label>
            <textarea class="lattice-input" id="template-desc" bind:value={description} placeholder="What is this template for?" rows="2"></textarea>
        </div>

        <div class="form-row">
            <div class="form-group">
                <label for="template-mode">Session Mode</label>
                <select class="lattice-input" id="template-mode" bind:value={mode}>
                    <option value="hive">Hive</option>
                    <option value="fusion">Fusion</option>
                    <option value="debate">Debate</option>
                </select>
            </div>
            <div class="form-group">
                <label for="template-strategy">Workspace Strategy</label>
                {#if mode === 'hive'}
                    <select class="lattice-input" id="template-strategy" bind:value={workspace_strategy}>
                        <option value="shared_cell">Shared Cell</option>
                        <option value="isolated_cell">Isolated Cell</option>
                    </select>
                {:else}
                    <div id="template-strategy" class="fixed-value">
                        Isolated worktrees (fixed for {mode === 'fusion' ? 'Fusion' : 'Debate'})
                    </div>
                {/if}
            </div>
        </div>
    </div>

    <div class="cells-section">
        <div class="section-header">
            <h4>Cells ({cells.length})</h4>
            <button type="button" class="lattice-btn lattice-btn--secondary lattice-btn--compact" on:click={addCell}>+ Add Cell</button>
        </div>

        <div class="cells-list lattice-scroll-chrome">
            {#each cells as cell, i}
                <div class="cell-editor-card lattice-panel">
                    <div class="card-header">
                        <span class="cell-num">Cell {i + 1}</span>
                        <button type="button" class="lattice-btn lattice-btn--ghost lattice-btn--danger lattice-btn--compact" on:click={() => removeCell(i)}>Remove</button>
                    </div>
                    
                    <div class="form-row">
                        <div class="form-group">
                            <label for="cell-role-{i}">Role</label>
                            <input class="lattice-input" id="cell-role-{i}" type="text" bind:value={cell.role} placeholder="e.g. backend" />
                        </div>
                        <div class="form-group">
                            <label for="cell-cli-{i}">CLI</label>
                            <input class="lattice-input" id="cell-cli-{i}" type="text" bind:value={cell.cli} placeholder="e.g. claude" />
                        </div>
                    </div>

                    <div class="form-group">
                        <label for="cell-model-{i}">Model (optional)</label>
                        <input class="lattice-input" id="cell-model-{i}" type="text" bind:value={cell.model} placeholder="e.g. opus" />
                    </div>

                    <div class="form-group">
                        <label for="cell-prompt-{i}">Prompt Template Key</label>
                        <input class="lattice-input" id="cell-prompt-{i}" type="text" bind:value={cell.prompt_template} placeholder="e.g. backend" />
                    </div>
                </div>
            {/each}
        </div>
    </div>

    <div class="actions lattice-forced-colors-boundary">
        <button type="button" class="lattice-btn lattice-btn--secondary" on:click={handleCancel}>Cancel</button>
        <button type="button" class="lattice-btn lattice-btn--primary" on:click={handleSave} disabled={!name}>Save Template</button>
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
        box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent-cyan) 20%, transparent);
    }

    .error-banner {
        padding: 10px 12px;
        border-radius: var(--radius-sm);
        background: color-mix(in srgb, var(--status-error) 12%, transparent);
        box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--status-error) 35%, transparent);
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
        width: 100%;
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
        border-radius: var(--radius-md);
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

    .actions {
        display: flex;
        justify-content: flex-end;
        gap: 12px;
        margin-top: 12px;
        padding-top: 20px;
        box-shadow: var(--edge-seam-top);
    }

</style>
