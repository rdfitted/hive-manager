<script lang="ts">
    import { cells } from '../../stores/cells';
    import ArtifactSummary from '../artifacts/ArtifactSummary.svelte';
    import Skeleton from '../Skeleton.svelte';
    import SkelBar from '../SkelBar.svelte';
    import { Hourglass, Wrench, Rocket, Lightning, NotePencil, CheckCircle, Question, XCircle, Skull } from 'phosphor-svelte';
    export let sessionId: string;

    $: sessionCells = Object.values($cells.cells).filter(c => c.session_id === sessionId);
    $: candidates = sessionCells.filter(c => c.cell_type !== 'resolver');

    const statusIcons: Record<string, any> = {
        'queued': Hourglass,
        'preparing': Wrench,
        'launching': Rocket,
        'running': Lightning,
        'summarizing': NotePencil,
        'completed': CheckCircle,
        'waiting_input': Question,
        'failed': XCircle,
        'killed': Skull
    };

    function getStatusClass(status: string): string {
        if (status === 'completed') return 'status-success';
        if (status === 'failed') return 'status-error';
        if (status === 'waiting_input') return 'status-warning';
        if (status === 'killed') return 'status-canceled';
        if (status === 'queued') return 'status-queued';
        return 'status-running';
    }
</script>

{#snippet artifactSkeleton()}
    <div class="artifact-skeleton">
        <SkelBar width="38%" height="0.7rem" />
        <SkelBar width="100%" height="0.65rem" />
        <SkelBar width="82%" height="0.65rem" />
        <SkelBar width="64%" height="0.65rem" />
    </div>
{/snippet}

<div class="fusion-comparison-view lattice-scroll-content">
    <div class="grid" style="grid-template-columns: repeat({Math.max(1, candidates.length)}, 1fr);">
        {#each candidates as cell (cell.id)}
            <div class="candidate-card lattice-panel" class:completed={cell.status === 'completed'} class:failed={cell.status === 'failed'}>
                <div class="card-header">
                    <div class="status-row">
                        <span class="status-badge {getStatusClass(cell.status)}" title={cell.status}>
                            <svelte:component 
                                this={statusIcons[cell.status] || Question} 
                                size={12} 
                                weight={cell.status === 'completed' || cell.status === 'failed' ? 'fill' : 'light'} 
                            />
                            {cell.status}
                        </span>
                        <span class="type-tag">{cell.cell_type}</span>
                    </div>

                    <h3 class="name">{cell.name}</h3>
                    <div class="branch-info">
                        <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/></svg>
                        {cell.workspace.branch_name}
                    </div>
                </div>

                <div class="card-content">
                    <Skeleton
                        loading={!cell.artifacts && (cell.status === 'running' || cell.status === 'summarizing')}
                        skeleton={artifactSkeleton}
                        class="artifact-loading"
                    >
                        {#if cell.artifacts}
                            <ArtifactSummary artifact={cell.artifacts} />
                        {:else}
                            <div class="empty-state">
                                No artifacts available.
                            </div>
                        {/if}
                    </Skeleton>
                </div>
            </div>
        {/each}
    </div>
</div>

<style>
    .fusion-comparison-view {
        width: 100%;
        height: 100%;
        overflow-x: auto;
        padding: 16px;
        background: var(--bg-chrome);
        border-radius: var(--radius-shell);
        box-shadow: var(--edge-lip);
    }

    .grid {
        display: grid;
        gap: 16px;
        min-width: min-content;
    }

    .candidate-card {
        display: flex;
        flex-direction: column;
        min-width: 320px;
        max-width: 500px;
        transition: background-color var(--motion-duration-standard) var(--motion-ease-standard);
    }

    .candidate-card.completed {
        /* Candidate outcome tint is semantic state, layered over the structural panel. */
        background: color-mix(in srgb, var(--status-success) 4%, var(--bg-surface));
    }

    .candidate-card.failed {
        /* Candidate outcome tint is semantic state, layered over the structural panel. */
        background: color-mix(in srgb, var(--status-error) 4%, var(--bg-surface));
    }

    .card-header {
        padding: 16px;
        border-bottom: 1px solid color-mix(in srgb, var(--text-primary) 5%, transparent);
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .status-row {
        display: flex;
        justify-content: space-between;
        align-items: center;
    }

    .type-tag {
        font-size: 9px;
        text-transform: uppercase;
        letter-spacing: 0.1em;
        color: var(--text-disabled);
        font-weight: 800;
    }

    .name {
        margin: 0;
        font-size: 18px;
        font-weight: 600;
        color: var(--text-primary);
    }

    .branch-info {
        display: flex;
        align-items: center;
        gap: 6px;
        font-size: 11px;
        color: var(--text-secondary);
        font-family: var(--font-mono);
    }

    .card-content {
        padding: 16px;
        flex: 1;
        display: flex;
        flex-direction: column;
        gap: 16px;
    }

    .artifact-skeleton {
        display: flex;
        flex-direction: column;
        gap: 10px;
        padding: 12px 0;
    }

    .empty-state {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        padding: 40px 20px;
        color: var(--text-disabled);
        font-size: 13px;
        text-align: center;
        gap: 12px;
    }
</style>
