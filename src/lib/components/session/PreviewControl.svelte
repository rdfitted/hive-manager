<script lang="ts">
    import { invoke } from '@tauri-apps/api/core';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { writeText } from '@tauri-apps/plugin-clipboard-manager';
    import { onDestroy, onMount } from 'svelte';
    import { ArrowClockwise, ArrowSquareOut, Browser, Columns, Copy, X } from 'phosphor-svelte';
    import { activeSession } from '../../stores/sessions';
    import { Popover } from 'bits-ui';
    import { IconButton } from '../ui';
    import Skeleton from '../Skeleton.svelte';
    import SkelBar from '../SkelBar.svelte';

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
    let selectionGeneration = 0;
    let previewRequestId = 0;
    let liveStatusRevision = 0;

    let previewOpen = false;
    let previewDocked = false;
    let previewCurrentUrl = '';
    // The preview skeleton must resolve on the STATUS request settling, not on the
    // URL being non-empty: an open preview with no URL yet is a legitimate terminal
    // state, and gating on emptiness alone leaves the skeleton spinning forever.
    let previewStatusResolved = false;
    let previewBusy = '';
    // Identity token for `previewBusy`, same shape as the poll guard in PR #154.
    // Neither an ungated nor a request-gated clear is correct on its own: ungated
    // lets an older transition's `finally` wipe a NEWER one's claim and re-enable
    // the dock controls mid-flight, while gating on `previewRequestId` strands the
    // controls disabled when no newer request follows. Each claimer takes a
    // generation and clears only if it still owns it.
    let previewBusyGeneration = 0;
    let previewUrlCopied = false;

    // Last URL previewed for each session, so switching sessions and coming back
    // does not make the operator retype it. Seeded from the backend, which
    // persists the same mapping across app restarts.
    const rememberedPreviewUrls = new Map<string, string>();

    let unlisteners: UnlistenFn[] = [];
    let destroyed = false;
    let copyResetTimer: ReturnType<typeof setTimeout> | undefined;

    function clearCopyFeedback() {
        previewUrlCopied = false;
        if (copyResetTimer) clearTimeout(copyResetTimer);
        copyResetTimer = undefined;
    }

    function setCurrentPreviewUrl(url: string) {
        if (url !== previewCurrentUrl) clearCopyFeedback();
        previewCurrentUrl = url;
    }

    function applyLiveStatus(status: PreviewStatus) {
        liveStatusRevision += 1;
        previewOpen = status.open;
        previewDocked = status.docked;
        setCurrentPreviewUrl(status.url ?? '');
        previewStatusResolved = true;
    }

    function rememberSessionUrl(sessionId: string | null, url: string | null) {
        if (sessionId && url) {
            rememberedPreviewUrls.set(sessionId, url);
            if (!previewUrl) previewUrl = url;
        }
    }

    function selectionIsCurrent(generation: number) {
        return !destroyed && generation === selectionGeneration;
    }

    async function refreshPreviewStatus(sessionId: string | null) {
        const generation = selectionGeneration;
        const revision = liveStatusRevision;
        try {
            const status = await invoke<PreviewStatus>('get_preview_status', { sessionId });
            if (!status || !selectionIsCurrent(generation)) return;
            if (revision === liveStatusRevision) applyLiveStatus(status);
            // Global events know the live URL, but only this probe can seed an uncached session URL.
            if (sessionId && !rememberedPreviewUrls.has(sessionId)) {
                rememberSessionUrl(sessionId, status.session_url);
            }
        } catch {
            // A status probe failing must never block the header from rendering.
        }
    }

    function retainListener(registration: Promise<UnlistenFn>) {
        void registration.then((unlisten) => {
            if (destroyed) unlisten();
            else unlisteners.push(unlisten);
        }).catch(() => {});
    }

    onMount(() => {
        retainListener(listen<{ url: string }>('preview-navigated', (event) => {
            if (!event.payload?.url || destroyed) return;
            liveStatusRevision += 1;
            setCurrentPreviewUrl(event.payload.url);
            previewOpen = true;
            previewStatusResolved = true;
        }));
        retainListener(listen<PreviewStatus>('preview-status', (event) => {
            if (event.payload && !destroyed) applyLiveStatus(event.payload);
        }));
    });

    onDestroy(() => {
        destroyed = true;
        unlisteners.forEach((fn) => fn());
        unlisteners = [];
        clearCopyFeedback();
    });

    $: session = $activeSession;
    $: if ((session?.id ?? null) !== previewSessionId) {
        selectionGeneration += 1;
        previewRequestId += 1;
        previewSessionId = session?.id ?? null;
        previewExpanded = false;
        // Per-session recall rather than an unconditional clear.
        previewUrl = previewSessionId ? rememberedPreviewUrls.get(previewSessionId) ?? '' : '';
        previewError = '';
        clearCopyFeedback();
        openingPreview = false;
        void refreshPreviewStatus(previewSessionId);
    }

    async function openPreview(event: SubmitEvent) {
        event.preventDefault();
        const url = previewUrl.trim();
        if (!url || openingPreview) return;

        const requestId = ++previewRequestId;
        const sessionId = previewSessionId;
        openingPreview = true;
        // Also claim `previewBusy`, which is what disables the dock/pop-out
        // cluster. `open_preview_window` can itself auto-dock, so an open and a
        // dock are two dock transitions - and the cluster becomes clickable the
        // moment the `preview-navigated` event fires during window construction,
        // while this call is still ahead of its own dock. Gating open on
        // `openingPreview` alone therefore left a window in which a stray click
        // could start a second transition. The backend serializes them
        // regardless (DOCK_TRANSITION); this just stops the UI offering it.
        previewBusy = 'open';
        const busyGeneration = ++previewBusyGeneration;
        previewError = '';
        try {
            const status = await invoke<PreviewStatus>('open_preview_window', { url, sessionId });
            if (!destroyed && requestId === previewRequestId) {
                if (sessionId) rememberedPreviewUrls.set(sessionId, url);
                if (status) {
                    applyLiveStatus(status);
                    rememberSessionUrl(sessionId, status.session_url);
                }
            }
        } catch (error) {
            if (requestId === previewRequestId) {
                previewError = String(error);
            }
        } finally {
            if (requestId === previewRequestId) {
                openingPreview = false;
            }
            // Ownership-gated, NOT request-gated. The session-change block
            // resets `openingPreview` but not `previewBusy`, so a newer
            // `openPreview` can start and claim the flag while this one is
            // still in flight; an unconditional clear here would then re-enable
            // the dock cluster mid-transition. Whoever holds the newest
            // generation clears it, so it is never stranded either.
            if (busyGeneration === previewBusyGeneration) {
                previewBusy = '';
            }
        }
    }

    async function runPreviewAction(command: string, key: string) {
        if (previewBusy) return;
        const sessionId = previewSessionId;
        const generation = selectionGeneration;
        previewBusy = key;
        const busyGeneration = ++previewBusyGeneration;
        previewError = '';
        try {
            const status = await invoke<PreviewStatus>(command, { sessionId });
            // The response carries a saved URL for this selection. Keep it scoped
            // without invalidating openPreview's independent request ID.
            if (status && selectionIsCurrent(generation) && busyGeneration === previewBusyGeneration) {
                applyLiveStatus(status);
                rememberSessionUrl(sessionId, status.session_url);
            }
        } catch (error) {
            if (selectionIsCurrent(generation) && busyGeneration === previewBusyGeneration) previewError = String(error);
        } finally {
            // Ownership-gated, like openPreview. `openPreview` does NOT check
            // `previewBusy` before claiming it (it guards on `openingPreview`),
            // so it can take the flag out from under an in-flight action here;
            // clearing unconditionally would then release a claim this call no
            // longer owns. previewBusy still needs no reset on the
            // session-change path — the newest owner always clears it.
            if (busyGeneration === previewBusyGeneration) {
                previewBusy = '';
            }
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
        const generation = selectionGeneration;
        previewBusy = 'reload';
        const busyGeneration = ++previewBusyGeneration;
        previewError = '';
        try {
            await invoke('reload_preview_window');
        } catch (error) {
            if (selectionIsCurrent(generation) && busyGeneration === previewBusyGeneration) previewError = String(error);
        } finally {
            // Ownership-gated for the same reason as the two above.
            if (busyGeneration === previewBusyGeneration) {
                previewBusy = '';
            }
        }
    }

    async function copyPreviewUrl() {
        const url = previewCurrentUrl;
        if (!url) return;
        const generation = selectionGeneration;
        const canShowFeedback = () => selectionIsCurrent(generation) && url === previewCurrentUrl;
        try {
            await writeText(url);
            if (!canShowFeedback()) return;
            clearCopyFeedback();
            previewUrlCopied = true;
            copyResetTimer = setTimeout(clearCopyFeedback, 1400);
        } catch (error) {
            if (canShowFeedback()) previewError = String(error);
        }
    }

    function clearPreviewError() {
        previewError = '';
    }
</script>

{#if session}
    <Popover.Root bind:open={previewExpanded}>
        <Popover.Trigger>
            {#snippet child({ props })}
                <IconButton {...props} class="preview-toggle" size="sm" icon={Browser} label="Preview"
                    tooltip="Preview a dev server or pull request URL" aria-haspopup="false" />
            {/snippet}
        </Popover.Trigger>
        <Popover.Content class="preview-popover" role="region" aria-label="Preview" align="end" sideOffset={8}
            onOpenAutoFocus={(event) => { event.preventDefault(); previewInput?.focus(); }}>
            <div class="preview-control">
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
                        class="preview-url-input lattice-input"
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
                        class="preview-open lattice-btn lattice-btn--secondary lattice-btn--compact"
                        type="submit"
                        disabled={openingPreview || !previewUrl.trim()}
                    >
                        <Browser size={15} weight="light" aria-hidden="true" />
                        {openingPreview ? 'Opening…' : 'Open'}
                    </button>
                </form>
                {#if previewOpen}
                    {@const dockLabel = previewDocked
                        ? 'Pop the preview out into a free-floating window'
                        : 'Dock the preview beside Hive Manager'}
                    <div class="preview-live lattice-forced-colors-boundary" aria-live="polite">
                        <span class="preview-live-mode">{previewDocked ? 'Docked' : 'Popped out'}</span>
                        <div class="preview-live-url" title={previewCurrentUrl}>
                            <Skeleton loading={!previewStatusResolved && !previewUrlCopied} layout="inline">
                                {#snippet skeleton()}
                                    <SkelBar width="12rem" height="1em" />
                                {/snippet}
                                {#snippet children()}
                                    {previewUrlCopied ? 'Copied' : (previewCurrentUrl || 'No preview URL')}
                                {/snippet}
                            </Skeleton>
                        </div>
                        <IconButton class="preview-action" size="sm" icon={Copy}
                            label="Copy the current preview URL" tooltip="Copy the current preview URL"
                            onclick={copyPreviewUrl} disabled={!previewCurrentUrl} />
                        <IconButton class="preview-action" size="sm" icon={ArrowClockwise}
                            label="Reload the preview" tooltip="Reload the preview"
                            onclick={reloadPreview} disabled={Boolean(previewBusy)} />
                        <IconButton class="preview-action" size="sm" icon={previewDocked ? ArrowSquareOut : Columns}
                            label={dockLabel} tooltip={dockLabel}
                            onclick={togglePreviewDock} disabled={Boolean(previewBusy)} />
                        <IconButton class="preview-action" size="sm" icon={X}
                            label="Close the preview" tooltip="Close the preview"
                            onclick={closePreview} disabled={Boolean(previewBusy)} />
                    </div>
                {/if}

                {#if previewError}
                    <span id="session-preview-error" class="preview-error" role="alert">{previewError}</span>
                {/if}
            </div>
        </Popover.Content>
    </Popover.Root>
{/if}

<style>
    :global(.preview-popover) {
        z-index: 1;
        max-width: calc(100vw - 2 * var(--space-2));
        padding: var(--space-3);
        border: 1px solid var(--line-strong);
        border-radius: var(--radius-lg);
        background: var(--surface-overlay);
        box-shadow: var(--elev-2);
        color: var(--text-primary);
        pointer-events: auto;
    }

    @media (forced-colors: active) {
        :global(.preview-popover) { border-color: CanvasText; }
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

    .preview-url-input {
        width: clamp(190px, 22vw, 320px);
        height: 30px;
    }

    .preview-url-input::placeholder {
        color: var(--text-disabled);
    }

    .preview-live {
        display: flex;
        align-items: center;
        gap: 4px;
        max-width: 100%;
        min-height: var(--control-height-sm);
        padding: var(--space-1) var(--space-2);
        box-shadow: var(--edge-lip);
        border-radius: var(--radius-sm);
        /* Live preview metadata is a compact control chip, not a structural panel. */
        background: var(--surface-overlay);
    }

    .preview-live-mode {
        flex-shrink: 0;
        font-size: 11px;
        font-weight: 600;
        color: var(--accent);
    }

    .preview-live-url {
        min-width: 0;
        flex: 1;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-family: var(--font-mono);
        font-size: 10px;
        color: var(--ink-muted);
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

</style>
