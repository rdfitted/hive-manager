<script lang="ts">
    import { invoke } from '@tauri-apps/api/core';
    import { tick } from 'svelte';
    import { Browser, GitBranch } from 'phosphor-svelte';
    import { activeSession, serdeEnumVariantName } from '../../stores/sessions';

    let previewExpanded = false;
    let previewUrl = '';
    let previewError = '';
    let openingPreview = false;
    let previewInput: HTMLInputElement;
    let previewSessionId: string | null = null;
    let previewRequestId = 0;

    $: session = $activeSession;
    $: mode = session ? serdeEnumVariantName(session.session_type)?.toLowerCase() ?? 'session' : 'session';
    $: status = session ? serdeEnumVariantName(session.state)?.toLowerCase() ?? 'unknown' : 'unknown';

    function truncatePath(path: string, maxLen = 44): string {
        const p = path.trim();
        if (p.length <= maxLen) return p;
        const head = Math.floor(maxLen / 2) - 1;
        const tail = maxLen - head - 1;
        return `${p.slice(0, Math.max(head, 1))}…${p.slice(-Math.max(tail, 1))}`;
    }

    $: wtPath = session?.worktree_path?.trim() ?? '';
    $: wtBranch = session?.worktree_branch?.trim() ?? '';
    $: worktreeTooltip = [wtBranch && `Branch: ${wtBranch}`, wtPath && `Path: ${wtPath}`].filter(Boolean).join('\n');
    $: worktreeChipLabel = wtBranch || (wtPath ? truncatePath(wtPath) : '');
    $: showWorktreeChip = Boolean(wtBranch || wtPath);

    $: if ((session?.id ?? null) !== previewSessionId) {
        previewRequestId += 1;
        previewSessionId = session?.id ?? null;
        previewExpanded = false;
        previewUrl = '';
        previewError = '';
        openingPreview = false;
    }

    async function expandPreview() {
        previewExpanded = true;
        previewError = '';
        await tick();
        previewInput?.focus();
    }

    async function openPreview(event: SubmitEvent) {
        event.preventDefault();
        const url = previewUrl.trim();
        if (!url || openingPreview) return;

        const requestId = ++previewRequestId;
        openingPreview = true;
        previewError = '';
        try {
            await invoke('open_preview_window', { url });
        } catch (error) {
            if (requestId === previewRequestId) {
                previewError = String(error);
            }
        } finally {
            if (requestId === previewRequestId) {
                openingPreview = false;
            }
        }
    }

    function clearPreviewError() {
        previewError = '';
    }
</script>

<div class="session-header">
    {#if session}
        <div class="main-info">
            <div class="top-row">
                <span class="mode-badge {mode}">{mode}</span>
                <h1 class="session-name">{session.name || session.id}</h1>
                <span class="status-badge {status}">{status.replaceAll('_', ' ')}</span>
                {#if showWorktreeChip}
                    <span class="worktree-chip" title={worktreeTooltip}>
                        <GitBranch size={14} weight="light" aria-hidden="true" />
                        <span class="worktree-label">{worktreeChipLabel}</span>
                    </span>
                {/if}
            </div>
            <p class="objective">Session ID: {session.id}</p>
        </div>

        <div class="stats">
            <div class="preview-control">
                {#if previewExpanded}
                    <form class="preview-form" onsubmit={openPreview}>
                        <label class="sr-only" for="session-preview-url">Preview URL</label>
                        <input
                            id="session-preview-url"
                            bind:this={previewInput}
                            bind:value={previewUrl}
                            type="url"
                            inputmode="url"
                            autocomplete="url"
                            placeholder="http://localhost:5173 or PR URL"
                            oninput={clearPreviewError}
                            aria-invalid={previewError ? 'true' : undefined}
                            aria-describedby={previewError ? 'session-preview-error' : undefined}
                        />
                        <button
                            class="preview-open"
                            type="submit"
                            disabled={openingPreview || !previewUrl.trim()}
                        >
                            <Browser size={15} weight="light" aria-hidden="true" />
                            {openingPreview ? 'Opening…' : 'Open'}
                        </button>
                    </form>
                    {#if previewError}
                        <span id="session-preview-error" class="preview-error" role="alert">{previewError}</span>
                    {/if}
                {:else}
                    <button
                        class="preview-toggle"
                        type="button"
                        onclick={expandPreview}
                        aria-expanded="false"
                        aria-label="Enter a URL to open in the isolated preview window"
                        title="Preview a dev server or pull request URL"
                    >
                        <Browser size={16} weight="light" aria-hidden="true" />
                        Preview
                    </button>
                {/if}
            </div>
            <div class="stat">
                <span class="label">Project</span>
                <span class="value">{session.project_path.split(/[\\\/]/).pop()}</span>
            </div>
            <div class="stat">
                <span class="label">Created</span>
                <span class="value">{new Date(session.created_at).toLocaleString()}</span>
            </div>
        </div>
    {:else}
        <div class="placeholder">Select a session to view details</div>
    {/if}
</div>

<style>
    .session-header {
        background: var(--bg-void);
        border-bottom: 1px solid var(--border-structural);
        padding: 12px 20px;
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
        gap: 32px;
    }

    .main-info {
        flex: 1;
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .top-row {
        display: flex;
        align-items: center;
        gap: 12px;
    }

    .session-name {
        margin: 0;
        font-size: 18px;
        font-weight: 600;
        color: var(--text-primary);
    }

    .mode-badge {
        font-size: 9px;
        font-weight: 800;
        text-transform: uppercase;
        padding: 2px 6px;
        border-radius: var(--radius-sm);
        letter-spacing: 0.1em;
        background: var(--bg-surface);
        color: var(--text-primary);
        border: 1px solid var(--border-structural);
    }

    .mode-badge.hive { background: var(--status-warning); color: var(--bg-void); }
    .mode-badge.fusion { background: var(--accent-cyan); color: var(--bg-void); }
    .mode-badge.swarm { background: var(--status-success); color: var(--bg-void); }
    .mode-badge.solo { background: var(--status-error); color: var(--bg-void); }

    .status-badge {
        font-size: 10px;
        padding: 2px 8px;
        border-radius: 999px;
        background: var(--bg-surface);
        color: var(--text-secondary);
        border: 1px solid var(--border-structural);
        text-transform: capitalize;
    }

    .status-badge.running { color: var(--status-running); border-color: var(--status-running); background: color-mix(in srgb, var(--status-running) 5%, transparent); }

    .worktree-chip {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        max-width: min(240px, 38vw);
        padding: 2px 8px;
        border-radius: var(--radius-sm);
        border: 1px solid var(--border-structural);
        background: var(--bg-elevated);
        color: var(--text-muted);
        font-size: 11px;
        font-family: var(--font-mono);
        white-space: nowrap;
    }

    .worktree-chip :global(svg) {
        flex-shrink: 0;
        color: var(--accent-cyan);
    }

    .worktree-label {
        overflow: hidden;
        text-overflow: ellipsis;
        min-width: 0;
    }

    .objective {
        margin: 0;
        font-size: 12px;
        color: var(--text-secondary);
        max-width: 800px;
        line-height: 1.4;
    }

    .stats {
        display: flex;
        align-items: flex-start;
        gap: 20px;
    }

    .preview-control {
        display: flex;
        flex-direction: column;
        align-items: flex-end;
        gap: 4px;
    }

    .preview-form {
        display: flex;
        align-items: center;
        gap: 6px;
    }

    .preview-form input {
        width: clamp(190px, 22vw, 320px);
        height: 30px;
        padding: 0 9px;
        border: 1px solid var(--border-structural);
        border-radius: var(--radius-sm);
        background: var(--bg-elevated);
        color: var(--text-primary);
        font-family: var(--font-mono);
        font-size: 11px;
    }

    .preview-form input::placeholder {
        color: var(--text-disabled);
    }

    .preview-form input:focus-visible {
        border-color: var(--accent-cyan);
        outline: 1px solid var(--accent-cyan);
        outline-offset: 1px;
    }

    .preview-toggle,
    .preview-open {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        gap: 6px;
        height: 30px;
        padding: 0 10px;
        border: 1px solid var(--border-structural);
        border-radius: var(--radius-sm);
        background: var(--bg-elevated);
        color: var(--text-secondary);
        font-size: 11px;
        font-weight: 600;
        cursor: pointer;
    }

    .preview-toggle:hover,
    .preview-toggle:focus-visible,
    .preview-open:hover:not(:disabled),
    .preview-open:focus-visible {
        border-color: var(--accent-cyan);
        color: var(--accent-cyan);
        outline: none;
    }

    .preview-open:disabled {
        cursor: not-allowed;
        opacity: 0.5;
    }

    .preview-error {
        max-width: 320px;
        color: var(--status-error);
        font-size: 10px;
        line-height: 1.3;
        text-align: right;
    }

    .sr-only {
        position: absolute;
        width: 1px;
        height: 1px;
        padding: 0;
        margin: -1px;
        overflow: hidden;
        clip: rect(0, 0, 0, 0);
        white-space: nowrap;
        border: 0;
    }

    .stat {
        display: flex;
        flex-direction: column;
        gap: 2px;
        align-items: flex-end;
    }

    .stat .label {
        font-size: 9px;
        text-transform: uppercase;
        color: var(--text-secondary);
        font-weight: 700;
        letter-spacing: 0.05em;
    }

    .stat .value {
        font-size: 11px;
        color: var(--text-primary);
        font-family: var(--font-mono);
    }

    .placeholder {
        color: var(--text-disabled);
        font-style: italic;
        padding: 8px 0;
    }

    @media (max-width: 1050px) {
        .session-header {
            flex-wrap: wrap;
            gap: 12px;
        }

        .stats {
            width: 100%;
            justify-content: flex-end;
        }
    }
</style>
