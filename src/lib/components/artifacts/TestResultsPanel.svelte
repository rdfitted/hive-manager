<script lang="ts">
    export let results: any;

    $: passed = results?.passed || 0;
    $: failed = results?.failed || 0;
    $: skipped = results?.skipped || 0;
    $: total = results?.total || (passed + failed + skipped);
    $: failures = results?.failures || [];
</script>

<div class="test-results-panel">
    <div class="summary">
        <div class="summary-item" class:pass={passed > 0}>
            <span class="count">{passed}</span>
            <span class="label">Passed</span>
        </div>
        <div class="summary-item" class:fail={failed > 0}>
            <span class="count">{failed}</span>
            <span class="label">Failed</span>
        </div>
        {#if skipped > 0}
            <div class="summary-item">
                <span class="count">{skipped}</span>
                <span class="label">Skipped</span>
            </div>
        {/if}
    </div>

    {#if failures && failures.length > 0}
        <div class="failures-list">
            <div class="failures-header">Failure Details</div>
            {#each failures as failure}
                <div class="failure-item">
                    <div class="test-name">
                        <span class="fail-icon">✗</span>
                        {failure.name || 'Unknown test'}
                    </div>
                    {#if failure.message}
                        <pre class="error-message">{failure.message}</pre>
                    {/if}
                </div>
            {/each}
        </div>
    {/if}
</div>

<style>
    .test-results-panel {
        display: flex;
        flex-direction: column;
        gap: 16px;
    }

    .summary {
        display: flex;
        gap: 12px;
    }

    .summary-item {
        flex: 1;
        padding: 8px;
        background: rgba(0, 0, 0, 0.2);
        border-radius: var(--radius-sm);
        display: flex;
        flex-direction: column;
        align-items: center;
        border: 1px solid rgba(255, 255, 255, 0.05);
    }

    .summary-item.pass {
        border-color: rgba(16, 185, 129, 0.2);
    }

    .summary-item.fail {
        border-color: rgba(239, 68, 68, 0.2);
    }

    .count {
        font-family: var(--font-mono);
        font-size: 18px;
        font-weight: 700;
        color: var(--text-primary);
    }

    .label {
        font-size: 10px;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        color: var(--text-secondary);
    }

    .summary-item.pass .count { color: var(--status-success); }
    .summary-item.fail .count { color: var(--status-error); }

    .failures-list {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .failures-header {
        font-size: 11px;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        color: var(--text-disabled);
        font-weight: 700;
    }

    .failure-item {
        background: rgba(239, 68, 68, 0.05);
        border: 1px solid rgba(239, 68, 68, 0.1);
        border-radius: var(--radius-sm);
        padding: 8px;
    }

    .test-name {
        font-size: 12px;
        color: var(--status-error);
        font-weight: 600;
        margin-bottom: 4px;
        display: flex;
        align-items: center;
        gap: 6px;
    }

    .fail-icon {
        font-size: 14px;
    }

    .error-message {
        margin: 0;
        font-family: var(--font-mono);
        font-size: 11px;
        color: var(--text-secondary);
        background: rgba(0, 0, 0, 0.2);
        padding: 6px;
        border-radius: 2px;
        overflow-x: auto;
    }
</style>
