<script lang="ts">
    import { Check, X, Question } from 'phosphor-svelte';
    export let results: any;

    $: passed = results?.passed || 0;
    $: failed = results?.failed || 0;
    $: total = results?.total || (passed + failed);
    $: status = failed > 0 ? 'fail' : (passed > 0 ? 'pass' : 'unknown');
</script>

<div class="status-badge {status === 'pass' ? 'status-success' : status === 'fail' ? 'status-error' : 'status-queued'}">
    {#if status === 'pass'}
        <Check size={12} weight="light" />
        <span class="text">Tests Passed</span>
    {:else if status === 'fail'}
        <X size={12} weight="light" />
        <span class="text">{failed}/{total} Failed</span>
    {:else}
        <Question size={12} weight="light" />
        <span class="text">No Tests</span>
    {/if}
</div>

