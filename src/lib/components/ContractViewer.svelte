<script lang="ts">
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
{:else}
  <div class="contract-viewer lattice-forced-colors-boundary">
    <p class="contract-empty">No sprint contract available.</p>
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

  .contract-empty {
    margin: 0;
    padding: 16px;
    color: var(--text-secondary);
    font-size: 13px;
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

</style>
