<script lang="ts">
    export let results: any;

    $: passed = results?.passed || 0;
    $: failed = results?.failed || 0;
    $: total = results?.total || (passed + failed);
    $: status = failed > 0 ? 'fail' : (passed > 0 ? 'pass' : 'unknown');
</script>

<div class="test-badge" class:pass={status === 'pass'} class:fail={status === 'fail'} class:unknown={status === 'unknown'}>
    {#if status === 'pass'}
        <span class="icon">✓</span>
        <span class="text">Tests Passed</span>
    {:else if status === 'fail'}
        <span class="icon">✗</span>
        <span class="text">{failed}/{total} Failed</span>
    {:else}
        <span class="icon">?</span>
        <span class="text">No Tests</span>
    {/if}
</div>

<style>
    .test-badge {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        padding: 2px 8px;
        border-radius: var(--radius-sm);
        font-size: 10px;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.05em;
    }

    .test-badge.pass {
        background: color-mix(in srgb, var(--status-success) 10%, transparent);
        color: var(--status-success);
        border: 1px solid color-mix(in srgb, var(--status-success) 20%, transparent);
    }

    .test-badge.fail {
        background: color-mix(in srgb, var(--status-error) 10%, transparent);
        color: var(--status-error);
        border: 1px solid color-mix(in srgb, var(--status-error) 20%, transparent);
    }

    .test-badge.unknown {
        background: color-mix(in srgb, var(--text-primary) 5%, transparent);
        color: var(--text-disabled);
        border: 1px solid color-mix(in srgb, var(--text-primary) 10%, transparent);
    }

    .icon {
        font-size: 12px;
    }
</style>
