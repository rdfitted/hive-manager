<script lang="ts">
  export let nodesComplete: number;
  export let nodesObserved: number;
  export let nodesInferred: number;
  export let nodesTotal: number;
  export let wavesComplete: number;
  export let wavesTotal: number;
  export let criticalPathRemaining: number;
</script>

<header
  class="progress-header lattice-forced-colors-boundary"
  aria-label="Work graph progress"
  aria-live="polite"
>
  <dl class="progress-list">
    <div class="progress-stat">
      <dt>Nodes completed</dt>
      <dd data-testid="nodes-progress">{nodesComplete} / {nodesTotal}</dd>
    </div>
    <div class="progress-stat progress-stat--observed">
      <dt>Direct evidence</dt>
      <dd data-testid="observed-progress">{nodesObserved}</dd>
    </div>
    <div class="progress-stat progress-stat--inferred">
      <dt>Lane inferred</dt>
      <dd data-testid="inferred-progress">{nodesInferred}</dd>
    </div>
    <div class="progress-stat">
      <dt>Waves completed</dt>
      <dd data-testid="waves-progress">{wavesComplete} / {wavesTotal}</dd>
    </div>
    <div class="progress-stat">
      <dt>Critical path remaining</dt>
      <dd data-testid="critical-path-remaining">{criticalPathRemaining}</dd>
    </div>
  </dl>
</header>

<style>
  .progress-header {
    display: flex;
    align-items: center;
    padding: var(--space-3) var(--space-4);
    color: var(--text-primary);
    background-color: var(--bg-chrome);
    border-radius: var(--radius-lg);
    box-shadow: var(--edge-lip), var(--edge-seam);
  }

  .progress-list {
    display: flex;
    flex: 1;
    align-items: center;
    gap: var(--space-5);
    margin: 0;
    padding: 0;
    flex-wrap: wrap;
  }

  .progress-stat {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
  }

  dt {
    color: var(--text-secondary);
    font-family: var(--font-body);
    font-size: var(--text-small);
    font-weight: 600;
  }

  dd {
    margin: 0;
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: var(--text-base);
    font-weight: 700;
    white-space: nowrap;
  }

  .progress-stat--observed dd,
  .progress-stat--inferred dd {
    color: var(--status-success);
  }

  .progress-stat--inferred dd {
    opacity: 0.75;
  }
</style>
