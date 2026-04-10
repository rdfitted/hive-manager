<script lang="ts">
    export let changedFiles: string[];
    export let diffSummary: string | undefined = undefined;

    // Basic parsing of diff summary if provided, otherwise just list files
    $: lines = diffSummary ? diffSummary.split('\n') : [];
</script>

<div class="diff-summary-panel">
    {#if diffSummary}
        <pre class="diff-summary-text">{diffSummary}</pre>
    {:else}
        <div class="files-list">
            {#each changedFiles as file}
                <div class="file-item">
                    <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>
                    <span class="file-path">{file}</span>
                </div>
            {/each}
        </div>
    {/if}
</div>

<style>
    .diff-summary-panel {
        background: rgba(0, 0, 0, 0.2);
        border-radius: var(--radius-sm);
        padding: 12px;
        border: 1px solid rgba(255, 255, 255, 0.05);
        overflow-x: auto;
    }

    .diff-summary-text {
        margin: 0;
        font-family: var(--font-mono);
        font-size: 11px;
        color: var(--text-secondary);
        line-height: 1.4;
    }

    .files-list {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .file-item {
        display: flex;
        align-items: center;
        gap: 8px;
        color: var(--text-primary);
    }

    .file-path {
        font-family: var(--font-mono);
        font-size: 11px;
    }
</style>
