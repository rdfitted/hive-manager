<script lang="ts">
    import type { ArtifactBundle } from '../../types/domain';
    import TestResultsBadge from './TestResultsBadge.svelte';
    import TestResultsPanel from './TestResultsPanel.svelte';
    import DiffSummaryPanel from './DiffSummaryPanel.svelte';

    export let artifact: ArtifactBundle | undefined;
    export let compact = false;
    export let showDetails = false;

    $: changedFilesCount = artifact?.changed_files?.length || 0;
    $: commitsCount = artifact?.commits?.length || 0;
    $: unresolvedCount = artifact?.unresolved_issues?.length || 0;
    $: confidence = artifact?.confidence !== undefined ? Math.round(artifact.confidence * 100) : null;
</script>

{#if artifact}
<div class="artifact-summary" class:compact class:show-details={showDetails}>
    <div class="header">
        <div class="main-stats">
            <div class="stat">
                <span class="value">{changedFilesCount}</span>
                <span class="label">Files</span>
            </div>
            <div class="stat">
                <span class="value">{commitsCount}</span>
                <span class="label">Commits</span>
            </div>
            {#if unresolvedCount > 0}
                <div class="stat warning">
                    <span class="value">{unresolvedCount}</span>
                    <span class="label">Issues</span>
                </div>
            {/if}
        </div>
        
        {#if artifact.test_results}
            <TestResultsBadge results={artifact.test_results} />
        {/if}

        {#if confidence !== null}
            <div class="confidence-box" title="Confidence: {confidence}%" aria-label="Confidence {confidence}%">
                <div class="confidence-bar">
                    <div class="fill" style="width: {confidence}%" class:high={confidence >= 80} class:mid={confidence >= 50 && confidence < 80} class:low={confidence < 50}></div>
                </div>
                <span class="confidence-text">{confidence}%</span>
            </div>
        {/if}
    </div>

    {#if !compact && artifact.summary}
        <div class="summary-text">
            {artifact.summary}
        </div>
    {/if}

    {#if !compact && showDetails}
        {#if artifact.test_results}
            <div class="detail-section">
                <div class="detail-header">Tests</div>
                <TestResultsPanel results={artifact.test_results} />
            </div>
        {/if}

        <div class="detail-section">
            <div class="detail-header">Changes</div>
            <DiffSummaryPanel changedFiles={artifact.changed_files} diffSummary={artifact.diff_summary} />
        </div>
    {/if}

    {#if !compact && !showDetails && artifact.changed_files && artifact.changed_files.length > 0}
        <div class="files-list">
            {#each artifact.changed_files.slice(0, 3) as file}
                <div class="file-item">
                    <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>
                    <span class="file-name">{file}</span>
                </div>
            {/each}
            {#if artifact.changed_files.length > 3}
                <div class="more-files">+{artifact.changed_files.length - 3} more files...</div>
            {/if}
            <button class="show-details-btn" on:click={() => showDetails = true}>Show detailed diff & tests</button>
        </div>
    {/if}
</div>
{/if}

<style>
    .artifact-summary {
        display: flex;
        flex-direction: column;
        gap: 12px;
        background: rgba(0, 0, 0, 0.2);
        padding: 12px;
        border-radius: 6px;
        border: 1px solid rgba(255, 255, 255, 0.05);
    }

    .artifact-summary.compact {
        padding: 8px;
        gap: 8px;
    }

    .header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
        flex-wrap: wrap;
    }

    .main-stats {
        display: flex;
        gap: 16px;
    }

    .stat {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
    }

    .stat.warning .value {
        color: #f59e0b;
    }

    .value {
        font-family: var(--font-mono);
        font-weight: 700;
        font-size: 14px;
        color: #fff;
    }

    .label {
        font-size: 9px;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        color: #666;
    }

    .confidence-box {
        display: flex;
        align-items: center;
        gap: 8px;
        min-width: 80px;
    }

    .confidence-bar {
        flex: 1;
        height: 4px;
        background: rgba(255, 255, 255, 0.1);
        border-radius: 2px;
        overflow: hidden;
    }

    .fill {
        height: 100%;
        transition: width 0.3s ease;
    }

    .fill.high { background: #10b981; }
    .fill.mid { background: #f59e0b; }
    .fill.low { background: #ef4444; }

    .confidence-text {
        font-size: 10px;
        font-weight: 700;
        color: #888;
        font-family: var(--font-mono);
    }

    .summary-text {
        font-size: 12px;
        color: #ccc;
        line-height: 1.5;
        display: -webkit-box;
        -webkit-line-clamp: 3;
        -webkit-box-orient: vertical;
        overflow: hidden;
    }

    .detail-section {
        display: flex;
        flex-direction: column;
        gap: 8px;
        margin-top: 8px;
        border-top: 1px solid rgba(255, 255, 255, 0.05);
        padding-top: 12px;
    }

    .detail-header {
        font-size: 10px;
        text-transform: uppercase;
        letter-spacing: 0.1em;
        color: #555;
        font-weight: 800;
    }

    .files-list {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .file-item {
        display: flex;
        align-items: center;
        gap: 6px;
        color: #888;
    }

    .file-name {
        font-size: 11px;
        font-family: var(--font-mono);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .more-files {
        font-size: 10px;
        color: #555;
        padding-left: 18px;
        font-style: italic;
    }

    .show-details-btn {
        margin-top: 8px;
        background: transparent;
        border: 1px solid rgba(255, 255, 255, 0.1);
        color: #888;
        font-size: 10px;
        padding: 4px 8px;
        border-radius: 4px;
        cursor: pointer;
    }

    .show-details-btn:hover {
        background: rgba(255, 255, 255, 0.05);
        color: #ccc;
    }
</style>
