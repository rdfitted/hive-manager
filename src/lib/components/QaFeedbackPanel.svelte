<script lang="ts">
  import { CaretDown, CaretUp, Check, X } from 'phosphor-svelte';
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { activeSession } from '$lib/stores/sessions';

  interface QaCriterion {
    id: string;
    label: string;
    passed: boolean;
    evidence: string | null;
    advisory_status: 'pass' | 'fail' | 'undetermined';
    advisory_threshold_disagreement: boolean;
    unchanged_evidence_flip: boolean;
  }

  interface QaVerdict {
    session_id: string;
    milestone_id: string;
    iteration: number;
    passed: boolean;
    advisory_verdict: 'pass' | 'fail' | 'undetermined';
    advisory_disagrees: boolean;
    advisory_threshold_disagreement: boolean;
    advisory_flip: boolean;
    criteria: QaCriterion[];
    summary: string;
    timestamp: string;
  }

  let verdict = $state<QaVerdict | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let collapsed = $state(false);

  async function loadVerdict(sessionId: string) {
    loading = true;
    error = null;
    try {
      // Use the new endpoint for QA verdict
      const data = await invoke<QaVerdict | null>('get_qa_verdict', { sessionId });
      verdict = data;
    } catch (e) {
      console.error('Failed to load QA verdict:', e);
      // It's okay if it doesn't exist yet
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    if ($activeSession) {
      loadVerdict($activeSession.id);
    }

    const unlisten = listen('qa-verdict-updated', (event) => {
      if ($activeSession && (event.payload as any).session_id === $activeSession.id) {
        loadVerdict($activeSession.id);
      }
    });

    return () => {
      unlisten.then(fn => fn());
    };
  });

  $effect(() => {
    if ($activeSession?.id) {
      loadVerdict($activeSession.id);
    } else {
      verdict = null;
    }
  });

  const stateClass = $derived(verdict ? (verdict.passed ? 'passed' : 'failed') : '');

  function advisoryLabel(status: QaVerdict['advisory_verdict']): string {
    return status === 'pass' ? 'Pass' : status === 'fail' ? 'Fail' : 'Undetermined';
  }

  function criterionDisagrees(criterion: QaCriterion): boolean {
    return criterion.advisory_status !== 'undetermined' && (criterion.advisory_status === 'pass') !== criterion.passed;
  }
</script>

{#if verdict}
  <div class="qa-feedback-panel lattice-panel" class:collapsed>
    <button class="lattice-btn lattice-btn--ghost lattice-btn--menu-item" onclick={() => collapsed = !collapsed}>
      <span class="header-content">
        <span class="header-left">
          <span class="status-icon {stateClass}">
            {#if verdict.passed}
              <Check size={14} weight="fill" />
            {:else}
              <X size={14} weight="fill" />
            {/if}
          </span>
          <h3>QA Feedback</h3>
          <span class="iteration-badge status-badge status-queued">Attempt {verdict.iteration}</span>
        </span>
        <span class="chevron">
          {#if collapsed}
            <CaretDown size={10} weight="light" />
          {:else}
            <CaretUp size={10} weight="light" />
          {/if}
        </span>
      </span>
    </button>

    {#if !collapsed}
      <div class="panel-content">
        <div
          class="verdict-comparison lattice-forced-colors-boundary"
          class:disagreement={verdict.advisory_disagrees}
          class:lattice-forced-colors-boundary--active={verdict.advisory_disagrees}
          role="group"
          aria-label="QA verdict comparison"
        >
          <div class="verdict-card" role="group" aria-label="Evaluator verdict">
            <span class="verdict-source">Evaluator</span>
            <span class="status-badge" class:status-success={verdict.passed} class:status-error={!verdict.passed}>
              {verdict.passed ? 'Pass' : 'Fail'}
            </span>
          </div>
          <div class="verdict-card" role="group" aria-label="Advisory verdict">
            <span class="verdict-source">Advisory</span>
            <span
              class="status-badge"
              class:status-success={verdict.advisory_verdict === 'pass'}
              class:status-error={verdict.advisory_verdict === 'fail'}
              class:status-queued={verdict.advisory_verdict === 'undetermined'}
            >
              {advisoryLabel(verdict.advisory_verdict)}
            </span>
          </div>
          {#if verdict.advisory_disagrees}
            <span class="disagreement-label">
              {verdict.advisory_threshold_disagreement ? 'Threshold disagreement' : 'Verdicts disagree'}
            </span>
          {/if}
          {#if verdict.advisory_flip}
            <span class="flip-label">Unchanged evidence, changed result</span>
          {/if}
        </div>

        <div class="summary lattice-forced-colors-boundary">
          <p>{verdict.summary}</p>
        </div>

        <div class="criteria-list" role="list" aria-label="QA criteria">
          {#each verdict.criteria as criterion}
            <div
              class="criterion-item lattice-forced-colors-boundary"
              class:passed={criterion.passed}
              class:disagreement={criterionDisagrees(criterion)}
              class:lattice-forced-colors-boundary--active={criterionDisagrees(criterion)}
              role="listitem"
            >
              <div class="criterion-header">
                <span class="criterion-icon">
                  {#if criterion.passed}
                    <Check size={12} weight="fill" />
                  {:else}
                    <X size={12} weight="fill" />
                  {/if}
                </span>
                <span class="criterion-label">
                  <span class="criterion-number">Criterion {criterion.id}</span>
                  {criterion.label}
                </span>
              </div>
              <div class="criterion-results">
                <span>Evaluator: {criterion.passed ? 'Pass' : 'Fail'}</span>
                <span>Advisory: {advisoryLabel(criterion.advisory_status)}</span>
                {#if criterion.advisory_threshold_disagreement}
                  <span class="criterion-flag">Threshold disagreement</span>
                {/if}
                {#if criterion.unchanged_evidence_flip}
                  <span class="criterion-flag">Unchanged evidence, changed result</span>
                {/if}
              </div>
              {#if criterion.evidence}
                <p class="criterion-evidence">{criterion.evidence}</p>
              {/if}
            </div>
          {/each}
        </div>
        
        <div class="footer">
          <span class="timestamp">{new Date(verdict.timestamp).toLocaleString()}</span>
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .qa-feedback-panel {
    margin-bottom: 12px;
    overflow: hidden;
  }

  .header-content {
    width: 100%;
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .status-icon {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .status-icon.passed {
    color: var(--status-success);
  }

  .status-icon.failed {
    color: var(--status-error);
  }

  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .iteration-badge {
    flex: 0 0 auto;
  }

  .chevron {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-secondary);
  }

  .panel-content {
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .verdict-comparison {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
    padding: 10px;
    border-radius: var(--radius-sm);
    box-shadow: var(--edge-seam);
  }

  .verdict-comparison.disagreement,
  .criterion-item.disagreement {
    box-shadow: inset 0 0 0 1px var(--status-warning);
  }

  .verdict-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-width: 0;
  }

  .verdict-source {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .disagreement-label,
  .flip-label {
    grid-column: 1 / -1;
    font-size: 11px;
    color: var(--status-warning);
  }

  .summary {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-primary);
    padding-bottom: 12px;
    box-shadow: var(--edge-seam);
  }

  .summary p {
    margin: 0;
  }

  .criteria-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .criterion-item {
    padding: 10px;
    border-radius: var(--radius-sm);
    background: color-mix(in srgb, var(--status-error) 5%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--status-error) 10%, transparent);
  }

  .criterion-item.passed {
    background: color-mix(in srgb, var(--status-success) 5%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--status-success) 10%, transparent);
  }

  .criterion-results {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    padding-left: 20px;
    font-size: 11px;
    color: var(--text-secondary);
  }

  .criterion-flag {
    color: var(--status-warning);
  }

  .criterion-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }

  .criterion-icon {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .criterion-item:not(.passed) .criterion-icon {
    color: var(--status-error);
  }

  .criterion-item.passed .criterion-icon {
    color: var(--status-success);
  }

  .criterion-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-primary);
  }

  .criterion-number {
    margin-right: 6px;
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
  }

  .criterion-evidence {
    margin: 0;
    font-size: 11px;
    color: var(--text-secondary);
    line-height: 1.4;
    padding-left: 20px;
  }

  .footer {
    display: flex;
    justify-content: flex-end;
    margin-top: 4px;
  }

  .timestamp {
    font-size: 10px;
    color: var(--text-secondary);
  }
</style>
