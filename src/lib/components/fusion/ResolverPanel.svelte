<script lang="ts">
    import { onMount } from 'svelte';
    import { artifacts } from '../../stores/artifacts';

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

<div class="resolver-panel">
    {#if output}
        <div class="output-card">
            <div class="card-header">
                <span class="label">Resolver Decision</span>
                <div class="selected-badge">
                    Selected: <span class="candidate-name">{output.selected_candidate}</span>
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
                <div class="section hybrid">
                    <h4>Hybrid Integration Plan</h4>
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
    {:else if loading}
        <div class="empty-state">
            <div class="spinner"></div>
            <span>Waiting for resolver output...</span>
        </div>
    {:else if error}
        <div class="error-state" role="alert">{error}</div>
    {:else}
        <div class="empty-state">
            <div class="icon" aria-hidden="true">Resolver</div>
            <span>Resolver has not run yet. Once all candidates complete, the Resolver will analyze and recommend the best variant.</span>
        </div>
    {/if}
</div>

<style>
    .resolver-panel {
        padding: 16px;
        height: 100%;
        overflow-y: auto;
    }

    .output-card {
        background: color-mix(in srgb, var(--accent-cyan) 5%, var(--bg-surface));
        border: 1px solid color-mix(in srgb, var(--accent-cyan) 20%, transparent);
        border-radius: var(--radius-sm);
        padding: 20px;
        display: flex;
        flex-direction: column;
        gap: 20px;
    }

    .card-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        border-bottom: 1px solid color-mix(in srgb, var(--accent-cyan) 10%, transparent);
        padding-bottom: 12px;
    }

    .label {
        font-size: 11px;
        text-transform: uppercase;
        letter-spacing: 0.1em;
        color: var(--accent-cyan);
        font-weight: 700;
    }

    .selected-badge {
        font-size: 13px;
        color: var(--text-secondary);
    }

    .candidate-name {
        font-weight: 700;
        color: var(--text-primary);
        background: color-mix(in srgb, var(--status-success) 20%, transparent);
        padding: 2px 8px;
        border-radius: var(--radius-sm);
        margin-left: 4px;
    }

    .section h4 {
        margin: 0 0 8px 0;
        font-size: 12px;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        color: var(--text-disabled);
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
        color: var(--text-secondary);
        font-size: 13px;
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .hybrid {
        background: color-mix(in srgb, var(--bg-void) 45%, var(--bg-surface));
        padding: 12px;
        border-radius: var(--radius-sm);
        border-left: 3px solid var(--status-warning);
    }

    .plan-content {
        font-size: 13px;
        color: var(--text-secondary);
        line-height: 1.5;
        white-space: pre-wrap;
    }

    .recommendation {
        margin-top: 12px;
        background: var(--accent-cyan);
        color: var(--bg-void);
        padding: 16px;
        border-radius: var(--radius-sm);
        box-shadow: 0 4px 12px color-mix(in srgb, var(--accent-cyan) 30%, transparent);
    }

    .rec-label {
        font-size: 10px;
        text-transform: uppercase;
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
        color: var(--text-disabled);
        text-align: center;
        gap: 16px;
        max-width: 400px;
        margin: 0 auto;
    }

    .error-state {
        margin: 0 auto;
        max-width: 480px;
        padding: 14px 16px;
        border-radius: var(--radius-sm);
        background: color-mix(in srgb, var(--status-error) 12%, transparent);
        border: 1px solid color-mix(in srgb, var(--status-error) 30%, transparent);
        color: var(--status-error);
        text-align: center;
    }

    .icon {
        font-size: 24px;
        font-weight: 700;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        opacity: 0.3;
    }

    .spinner {
        width: 32px;
        height: 32px;
        border: 2px solid color-mix(in srgb, var(--text-primary) 10%, transparent);
        border-top-color: var(--accent-cyan);
        border-radius: 50%;
        animation: spin 1s linear infinite;
    }

    @keyframes spin {
        to { transform: rotate(360deg); }
    }
</style>
