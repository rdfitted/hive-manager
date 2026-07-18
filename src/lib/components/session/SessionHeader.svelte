<script lang="ts">
    import { invoke } from '@tauri-apps/api/core';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { writeText } from '@tauri-apps/plugin-clipboard-manager';
    import { onDestroy, onMount, tick } from 'svelte';
    import { ArrowClockwise, ArrowSquareOut, Browser, Columns, Copy, GitBranch, X } from 'phosphor-svelte';
    import { activeSession, serdeEnumVariantName } from '../../stores/sessions';

    /** Mirrors `PreviewStatus` in src-tauri/src/preview/mod.rs. */
    type PreviewStatus = {
        open: boolean;
        docked: boolean;
        url: string | null;
        session_url: string | null;
    };

    let previewExpanded = false;
    let previewUrl = '';
    let previewError = '';
    let openingPreview = false;
    let previewInput: HTMLInputElement;
    let previewSessionId: string | null = null;
    let previewRequestId = 0;

    let previewOpen = false;
    let previewDocked = false;
    let previewCurrentUrl = '';
    let previewBusy = '';
    let previewUrlCopied = false;

    // Last URL previewed for each session, so switching sessions and coming back
    // does not make the operator retype it. Seeded from the backend, which
    // persists the same mapping across app restarts.
    const rememberedPreviewUrls = new Map<string, string>();

    let unlisteners: UnlistenFn[] = [];
    let destroyed = false;
    let copyResetTimer: ReturnType<typeof setTimeout> | undefined;

    function applyPreviewStatus(status: PreviewStatus | null, sessionId: string | null) {
        if (!status) return;
        previewOpen = status.open;
        previewDocked = status.docked;
        previewCurrentUrl = status.url ?? '';
        if (sessionId && status.session_url) {
            rememberedPreviewUrls.set(sessionId, status.session_url);
            if (!previewUrl) previewUrl = status.session_url;
        }
    }

    async function refreshPreviewStatus(sessionId: string | null) {
        try {
            const status = await invoke<PreviewStatus>('get_preview_status', { sessionId });
            if (sessionId === previewSessionId) applyPreviewStatus(status, sessionId);
        } catch {
            // A status probe failing must never block the header from rendering.
        }
    }

    onMount(() => {
        Promise.all([
            listen<{ url: string }>('preview-navigated', (event) => {
                if (!event.payload?.url) return;
                previewCurrentUrl = event.payload.url;
                previewOpen = true;
            }),
            listen<PreviewStatus>('preview-status', (event) => {
                if (!event.payload) return;
                previewOpen = event.payload.open;
                previewDocked = event.payload.docked;
                previewCurrentUrl = event.payload.url ?? '';
            })
        ])
            .then((fns) => {
                if (destroyed) {
                    fns.forEach((fn) => fn());
                    return;
                }
                unlisteners = fns;
            })
            .catch(() => {});
    });

    onDestroy(() => {
        destroyed = true;
        unlisteners.forEach((fn) => fn());
        unlisteners = [];
        if (copyResetTimer) clearTimeout(copyResetTimer);
    });

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
        // Per-session recall rather than an unconditional clear.
        previewUrl = previewSessionId ? rememberedPreviewUrls.get(previewSessionId) ?? '' : '';
        previewError = '';
        openingPreview = false;
        void refreshPreviewStatus(previewSessionId);
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
        const sessionId = previewSessionId;
        openingPreview = true;
        previewError = '';
        try {
            const status = await invoke<PreviewStatus>('open_preview_window', { url, sessionId });
            if (requestId === previewRequestId) {
                if (sessionId) rememberedPreviewUrls.set(sessionId, url);
                applyPreviewStatus(status, sessionId);
            }
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

    async function runPreviewAction(command: string, key: string) {
        if (previewBusy) return;
        const sessionId = previewSessionId;
        previewBusy = key;
        previewError = '';
        try {
            const status = await invoke<PreviewStatus>(command, { sessionId });
            // Freshness gate, mirroring refreshPreviewStatus: the payload is
            // session-scoped (status_for derives session_url from the caller's
            // session_id), so a response that lands after the operator switched
            // sessions would otherwise seed the OLD session's URL into the NEW
            // session's input via applyPreviewStatus.
            //
            // Deliberately NOT the `++previewRequestId` pattern used by
            // openPreview: bumping it from the dock/close path would invalidate
            // a concurrently in-flight openPreview and skip its
            // `openingPreview = false` reset, leaving the UI stuck busy.
            if (sessionId === previewSessionId) applyPreviewStatus(status, sessionId);
        } catch (error) {
            // Same gate as the success path: the session-change reactive block
            // clears previewError, so an ungated write here would replay the
            // OLD session's failure into the NEW session's header — announced
            // via role="alert" and flagging that session's URL input invalid.
            if (sessionId === previewSessionId) previewError = String(error);
        } finally {
            // Unconditional, unlike openPreview's gated reset — this is why
            // previewBusy needs no explicit clear on the session-change path
            // (and must not get one, or the re-entrancy guard above would let a
            // second action start while the first is still in flight).
            previewBusy = '';
        }
    }

    function togglePreviewDock() {
        return runPreviewAction(previewDocked ? 'undock_preview_window' : 'dock_preview_window', 'dock');
    }

    function closePreview() {
        return runPreviewAction('close_preview_window', 'close');
    }

    async function reloadPreview() {
        if (previewBusy) return;
        previewBusy = 'reload';
        previewError = '';
        try {
            await invoke('reload_preview_window');
        } catch (error) {
            previewError = String(error);
        } finally {
            previewBusy = '';
        }
    }

    async function copyPreviewUrl() {
        if (!previewCurrentUrl) return;
        try {
            await writeText(previewCurrentUrl);
            previewUrlCopied = true;
            if (copyResetTimer) clearTimeout(copyResetTimer);
            copyResetTimer = setTimeout(() => {
                previewUrlCopied = false;
            }, 1400);
        } catch (error) {
            previewError = String(error);
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
                        <!--
                            type="text", not type="url": native constraint validation
                            rejects scheme-less input before submit ever fires, which
                            would make the backend's forgiving normalization
                            unreachable. inputmode/autocomplete keep the URL keyboard
                            and autofill behaviour.
                        -->
                        <input
                            id="session-preview-url"
                            bind:this={previewInput}
                            bind:value={previewUrl}
                            type="text"
                            inputmode="url"
                            autocomplete="url"
                            spellcheck="false"
                            placeholder="localhost:5173, github.com/owner/repo"
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
                {:else}
                    <button
                        class="preview-toggle"
                        type="button"
                        onclick={expandPreview}
                        aria-expanded="false"
                        aria-label="Enter a URL to open in the isolated preview browser. The scheme is optional, so localhost:5173 or github.com/owner/repo both work."
                        title="Preview a dev server or pull request URL (the http:// is optional)"
                    >
                        <Browser size={16} weight="light" aria-hidden="true" />
                        Preview
                    </button>
                {/if}

                {#if previewOpen}
                    <div class="preview-live" aria-live="polite">
                        <span class="preview-live-mode">{previewDocked ? 'Docked' : 'Popped out'}</span>
                        <span class="preview-live-url" title={previewCurrentUrl}>
                            {previewUrlCopied ? 'Copied' : previewCurrentUrl || 'Loading…'}
                        </span>
                        <button
                            class="preview-action"
                            type="button"
                            onclick={copyPreviewUrl}
                            disabled={!previewCurrentUrl}
                            title="Copy the current preview URL"
                            aria-label="Copy the current preview URL"
                        >
                            <Copy size={13} weight="light" aria-hidden="true" />
                        </button>
                        <button
                            class="preview-action"
                            type="button"
                            onclick={reloadPreview}
                            disabled={Boolean(previewBusy)}
                            title="Reload the preview"
                            aria-label="Reload the preview"
                        >
                            <ArrowClockwise size={13} weight="light" aria-hidden="true" />
                        </button>
                        <button
                            class="preview-action"
                            type="button"
                            onclick={togglePreviewDock}
                            disabled={Boolean(previewBusy)}
                            title={previewDocked
                                ? 'Pop the preview out into a free-floating window'
                                : 'Dock the preview beside Hive Manager'}
                            aria-label={previewDocked
                                ? 'Pop the preview out into a free-floating window'
                                : 'Dock the preview beside Hive Manager'}
                        >
                            {#if previewDocked}
                                <ArrowSquareOut size={13} weight="light" aria-hidden="true" />
                            {:else}
                                <Columns size={13} weight="light" aria-hidden="true" />
                            {/if}
                        </button>
                        <button
                            class="preview-action"
                            type="button"
                            onclick={closePreview}
                            disabled={Boolean(previewBusy)}
                            title="Close the preview"
                            aria-label="Close the preview"
                        >
                            <X size={13} weight="light" aria-hidden="true" />
                        </button>
                    </div>
                {/if}

                {#if previewError}
                    <span id="session-preview-error" class="preview-error" role="alert">{previewError}</span>
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

    .preview-live {
        display: flex;
        align-items: center;
        gap: 4px;
        max-width: min(360px, 42vw);
        height: 24px;
        padding: 0 4px 0 8px;
        border: 1px solid var(--border-structural);
        border-radius: var(--radius-sm);
        background: var(--bg-elevated);
    }

    .preview-live-mode {
        flex-shrink: 0;
        font-size: 9px;
        font-weight: 800;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        color: var(--accent-cyan);
    }

    .preview-live-url {
        min-width: 0;
        flex: 1;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-family: var(--font-mono);
        font-size: 10px;
        color: var(--text-secondary);
    }

    .preview-action {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        flex-shrink: 0;
        width: 20px;
        height: 20px;
        padding: 0;
        border: none;
        border-radius: var(--radius-sm);
        background: transparent;
        color: var(--text-muted);
        cursor: pointer;
    }

    .preview-action:hover:not(:disabled),
    .preview-action:focus-visible {
        color: var(--accent-cyan);
        background: var(--bg-surface);
        outline: none;
    }

    .preview-action:disabled {
        cursor: not-allowed;
        opacity: 0.4;
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
