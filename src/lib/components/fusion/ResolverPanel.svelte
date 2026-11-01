<script lang="ts">
    import { onMount } from 'svelte';
    import { artifacts } from '../../stores/artifacts';
    import Skeleton from '../Skeleton.svelte';
    import SkelBar from '../SkelBar.svelte';

    export let sessionId: string;

    $: output = $artifacts.resolverOutputs[sessionId];
    $: loading = $artifacts.resolverLoading[sessionId] ?? false;
    $: error = $artifacts.resolverError[sessionId];

    onMount(() => {
        if (!output) {
            artifacts.fetchResolverOutput(sessionId);
        }
    });
</script>

{#snippet resolverSkeleton()}
    <div class="resolver-skeleton">
        <SkelBar width="42%" height="0.8rem" />
        <SkelBar width="100%" height="0.7rem" />
        <SkelBar width="88%" height="0.7rem" />
        <SkelBar width="100%" height="3.25rem" radius="md" />
    </div>
{/snippet}

<div class="resolver-panel lattice-scroll-content">
    <Skeleton loading={loading} skeleton={resolverSkeleton} class="resolver-loading">
        {#if output}
            <div class="output-card lattice-forced-colors-boundary">
                <div class="card-header lattice-forced-colors-boundary">
                    <span class="label">Resolver Decision</span>
                    <div class="selected-badge">
                        Selected: <span class="candidate-name status-badge status-success">{output.selected_candidate}</span>
                    </div>
                </div>

                <div class="section">
                    <h4>Rationale</h4>
                    <p class="rationale">{output.rationale}</p>
                </div>

                {#if output.tradeoffs && output.tradeoffs.length > 0}
                    <div class="section">
                        <h4>Trade-offs</h4>
                        <ul class="tradeoffs">
                            {#each output.tradeoffs as tradeoff}
                                <li>{tradeoff}</li>
                            {/each}
                        </ul>
                    </div>
                {/if}

                {#if output.hybrid_integration_plan}
                    <div class="section hybrid lattice-panel">
                        <h4>Hybrid Integration Plan <span class="status-badge status-warning lattice-forced-colors-boundary">Integration required</span></h4>
                        <div class="plan-content">
                            {output.hybrid_integration_plan}
                        </div>
                    </div>
                {/if}

                {#if output.final_recommendation}
                    <div class="recommendation">
                        <div class="rec-label">Final Recommendation</div>
                        <div class="rec-text">{output.final_recommendation}</div>
                    </div>
                {/if}
            </div>
        {:else if error}
            <div class="error-state lattice-forced-colors-boundary" role="alert">{error}</div>
        {:else}
            <div class="empty-state">
                <div class="icon" aria-hidden="true">Resolver</div>
                <span>Resolver has not run yet. Once all candidates complete, the Resolver will analyze and recommend the best variant.</span>
            </div>
        {/if}
    </Skeleton>
</div>

<style>
    .resolver-panel {
        padding: 16px;
        height: 100%;
        overflow-y: auto;
    }

    .output-card {
        padding: 20px;
        display: flex;
        flex-direction: column;
        gap: 20px;
        background: var(--chrome);
        border-radius: var(--radius-shell);
        box-shadow: var(--edge-lip);
    }

    .resolver-skeleton {
        display: flex;
        flex-direction: column;
        gap: 12px;
        padding: 20px;
    }

    .card-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        box-shadow: var(--edge-seam);
        padding-bottom: 12px;
    }

    .label {
        font-size: 11px;
        color: var(--accent);
        font-weight: 700;
    }

    .selected-badge {
        font-size: 13px;
        color: var(--text-muted);
    }

    .candidate-name {
        margin-left: 4px;
    }

    .section h4 {
        margin: 0 0 8px 0;
        font-size: 12px;
        color: var(--text-muted);
    }

    .rationale {
        margin: 0;
        font-size: 14px;
        color: var(--text-primary);
        line-height: 1.6;
    }

    .tradeoffs {
        margin: 0;
        padding-left: 20px;
        color: var(--text-muted);
        font-size: 13px;
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .hybrid {
        background: color-mix(in srgb, var(--status-warning) 8%, var(--surface));
        padding: 16px;
        border-radius: var(--radius-md);
    }

    .plan-content {
        font-size: 13px;
        color: var(--text-muted);
        line-height: 1.5;
        white-space: pre-wrap;
    }

    .recommendation {
        margin-top: 12px;
        /* Semantic recommendation treatment intentionally remains accent-filled. */
        background: var(--accent);
        color: var(--on-accent);
        padding: 16px;
        border-radius: var(--radius-md);
        box-shadow: var(--depth-1);
    }

    .rec-label {
        font-size: 10px;
        font-weight: 800;
        margin-bottom: 4px;
        opacity: 0.8;
    }

    .rec-text {
        font-size: 15px;
        font-weight: 600;
    }

    .empty-state {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        padding: 60px 20px;
        color: var(--text-muted);
        text-align: center;
        gap: 16px;
        max-width: 400px;
        margin: 0 auto;
    }

    .error-state {
        margin: 0 auto;
        max-width: 480px;
        padding: 14px 16px;
        border-radius: var(--radius-md);
        /* Semantic error treatment intentionally remains state-tinted. */
        background: color-mix(in srgb, var(--status-error) 12%, transparent);
        box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--status-error) 30%, transparent);
        color: var(--status-error);
        text-align: center;
    }

    .icon {
        font-size: 24px;
        font-weight: 700;
        opacity: 0.3;
    }

</style>
