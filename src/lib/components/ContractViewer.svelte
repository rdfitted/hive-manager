<script lang="ts">
  import { Check, X } from 'phosphor-svelte';
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { activeSession } from '$lib/stores/sessions';

  interface LegacyContract {
    id: string;
    milestone_id: string;
    content: string;
    passed: boolean | null;
    grading_weights: Record<string, number>;
    threshold: number;
  }

  type CompareOp = 'Lt' | 'Le' | 'Eq' | 'Ge' | 'Gt';
  type CriterionKind =
    | 'PassFail'
    | 'Unspecified'
    | { Scored: { min: number; max: number; floor: number | null } }
    | { Measured: { metric: string; op: CompareOp; target: number } };

  interface TypedCriterion {
    number: number;
    category: string | null;
    kind: CriterionKind;
    description: string;
  }

  interface SprintContract {
    milestone_name: string;
    acceptance_criteria: TypedCriterion[];
    pass_threshold: string[];
    threshold_policy:
      | {
          Rules: {
            require_all_pass_fail: boolean;
            scored_mean: { minimum: number; scale: number } | null;
            fail_scored_below_floor: boolean;
          };
        }
      | { Prose: string[] };
    warnings: string[];
    raw_markdown: string;
  }

  interface Props {
    contract?: SprintContract | null;
  }

  let { contract: typedContract = null }: Props = $props();

  let loadedContract = $state<LegacyContract | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function loadContract(sessionId: string) {
    loading = true;
    error = null;
    try {
      const data = await invoke<LegacyContract | null>('get_current_contract', { sessionId });
      loadedContract = data;
    } catch (e) {
      console.error('Failed to load contract:', e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    if (!typedContract && $activeSession) {
      loadContract($activeSession.id);
    }
  });

  $effect(() => {
    if (typedContract) {
      loadedContract = null;
    } else if ($activeSession?.id) {
      loadContract($activeSession.id);
    } else {
      loadedContract = null;
    }
  });

  const compareOpLabels: Record<CompareOp, string> = {
    Lt: '<',
    Le: '<=',
    Eq: '=',
    Ge: '>=',
    Gt: '>'
  };

  function criterionKindLabel(kind: CriterionKind): string {
    if (typeof kind === 'string') return kind;
    if ('Scored' in kind) {
      const { min, max, floor } = kind.Scored;
      return `Scored ${min}-${max}${floor === null ? '' : ` floor ${floor}`}`;
    }
    const { metric, op, target } = kind.Measured;
    return `Measured ${metric} ${compareOpLabels[op]} ${target}`;
  }
</script>

{#if typedContract}
  <div class="contract-viewer lattice-forced-colors-boundary">
    <div class="contract-header lattice-forced-colors-boundary">
      <div class="header-info">
        <h3>Sprint Contract</h3>
        <span class="milestone-id">Milestone: {typedContract.milestone_name}</span>
      </div>
    </div>

    <section class="typed-criteria" aria-label="Acceptance criteria">
      <h4>Acceptance Criteria</h4>
      <ol>
        {#each typedContract.acceptance_criteria as criterion (criterion.number)}
          <li>
            <span class="criterion-kind">{criterionKindLabel(criterion.kind)}</span>
            <span class="criterion-description">{criterion.description}</span>
          </li>
        {/each}
      </ol>
    </section>

    <section class="pass-threshold" aria-label="Pass threshold">
      <h4>Pass Threshold</h4>
      <ul>
        {#each typedContract.pass_threshold as threshold}
          <li>{threshold}</li>
        {/each}
      </ul>
    </section>
  </div>
{:else if loadedContract}
  <div class="contract-viewer lattice-forced-colors-boundary">
    <div class="contract-header lattice-forced-colors-boundary">
      <div class="header-info">
        <h3>Sprint Contract</h3>
        <span class="milestone-id">Milestone: {loadedContract.milestone_id}</span>
      </div>
      <div class="threshold-badge">
        Threshold: {loadedContract.threshold}%
      </div>
    </div>

    <div class="contract-content-wrapper lattice-scroll-content">
      {#if loadedContract.passed !== null}
        <div class="status-overlay" class:passed={loadedContract.passed}>
          <div class="overlay-icon">
            {#if loadedContract.passed}
              <Check size={80} weight="fill" />
            {:else}
              <X size={80} weight="fill" />
            {/if}
          </div>
          <div class="overlay-text">
            {loadedContract.passed ? 'PASSED' : 'FAILED'}
          </div>
        </div>
      {/if}
      
      <div class="content">
        <pre>{loadedContract.content}</pre>
      </div>
    </div>

    {#if Object.keys(loadedContract.grading_weights).length > 0}
      <div class="weights-section lattice-forced-colors-boundary">
        <h4>Grading Weights</h4>
        <div class="weights-grid">
          {#each Object.entries(loadedContract.grading_weights) as [key, weight]}
            <div class="weight-item lattice-panel">
              <span class="weight-key">{key}</span>
              <span class="weight-value">{weight}%</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .contract-viewer {
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--bg-chrome);
    border-radius: var(--radius-shell);
    box-shadow: var(--edge-lip);
  }

  .contract-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px;
    background: var(--bg-void);
    box-shadow: var(--edge-seam);
  }

  .header-info h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .milestone-id {
    font-size: 11px;
    color: var(--text-secondary);
  }

  .threshold-badge {
    font-size: 11px;
    padding: 3px 8px;
    background: var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
  }

  .contract-content-wrapper {
    position: relative;
    flex: 1;
    min-height: 200px;
    max-height: 400px;
    overflow-y: auto;
  }

  .status-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--status-error) 10%, transparent);
    pointer-events: none;
    z-index: 1;
  }

  .status-overlay.passed {
    background: color-mix(in srgb, var(--status-success) 10%, transparent);
  }

  .overlay-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0.3;
  }

  .status-overlay:not(.passed) .overlay-icon {
    color: var(--status-error);
  }

  .status-overlay.passed .overlay-icon {
    color: var(--status-success);
  }

  .overlay-text {
    font-size: 24px;
    font-weight: 800;
    letter-spacing: 4px;
    opacity: 0.3;
    margin-top: -10px;
  }

  .status-overlay:not(.passed) .overlay-text {
    color: var(--status-error);
  }

  .status-overlay.passed .overlay-text {
    color: var(--status-success);
  }

  .content {
    padding: 16px;
  }

  pre {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: var(--font-body);
    font-size: 13px;
    color: var(--text-primary);
    line-height: 1.5;
  }

  .weights-section {
    padding: 12px 16px;
    background: var(--bg-void);
    box-shadow: var(--edge-seam-top);
  }

  .typed-criteria,
  .pass-threshold {
    padding: 16px;
  }

  .pass-threshold {
    background: var(--bg-void);
    box-shadow: var(--edge-seam-top);
  }

  .typed-criteria ol,
  .pass-threshold ul {
    display: grid;
    gap: 8px;
    margin: 0;
    padding-left: 24px;
  }

  .typed-criteria li {
    color: var(--text-primary);
    font-size: 13px;
    line-height: 1.5;
  }

  .criterion-kind {
    display: inline-block;
    margin-right: 8px;
    padding: 2px 6px;
    border-radius: var(--radius-sm);
    background: var(--border-structural);
    color: var(--accent-cyan);
    font-family: var(--font-mono);
    font-size: 11px;
  }

  .criterion-description,
  .pass-threshold li {
    color: var(--text-primary);
    font-family: var(--font-body);
    font-size: 13px;
  }

  h4 {
    margin: 0 0 8px 0;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .weights-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: 8px;
  }

  .weight-item {
    display: flex;
    justify-content: space-between;
    padding: 4px 8px;
  }

  .weight-key {
    font-size: 11px;
    color: var(--text-primary);
  }

  .weight-value {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent-cyan);
  }
</style>
