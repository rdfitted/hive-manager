<script lang="ts">
  import type { CompletionProvenance } from '$lib/workgraph/types';

  const PROVENANCE_PRESENTATION: Record<
    CompletionProvenance,
    { readonly label: string; readonly detail: string }
  > = {
    declared: { label: 'Declared', detail: 'Explicit completion fact' },
    queue: { label: 'Queue', detail: 'Finalized queue record' },
    observed: { label: 'Observed', detail: 'Directly witnessed completion' },
    inferred: { label: 'Inferred', detail: 'Completed through lane fan-out' },
    plan: { label: 'Plan', detail: 'Persisted status without a backing queue row' }
  };

  interface NodeContract {
    readonly inputs: readonly string[];
    readonly outputs: readonly string[];
    readonly acceptance: readonly string[];
  }

  interface InspectorNode {
    readonly id: string;
    readonly title: string;
    readonly kind: string;
    readonly lane: string;
    readonly status: string;
    readonly contract: NodeContract;
  }

  interface ImmediateDependency {
    readonly id: string;
    readonly title: string;
    readonly kind: string;
  }

  interface InspectorProgress {
    readonly started_at: string | null;
    readonly finished_at: string | null;
    readonly attempts: number;
    readonly agent_id: string | null;
    readonly last_heartbeat_at: string | null;
  }

  interface Props {
    node: InspectorNode;
    dependencies: readonly ImmediateDependency[];
    progress?: InspectorProgress | null;
    completionProvenance?: CompletionProvenance | null;
    completionSourceRefs?: readonly string[];
  }

  let {
    node,
    dependencies,
    progress = null,
    completionProvenance = null,
    completionSourceRefs = []
  }: Props = $props();
  let hasContract = $derived(
    node.contract.inputs.length > 0 ||
      node.contract.outputs.length > 0 ||
      node.contract.acceptance.length > 0
  );
  let provenancePresentation = $derived(
    completionProvenance ? PROVENANCE_PRESENTATION[completionProvenance] : null
  );
</script>

<aside
  class="node-inspector lattice-panel lattice-forced-colors-boundary"
  aria-label={`Node inspector for ${node.title}`}
>
  <header class="inspector-header">
    <div class="badge-row">
      <span class="kind-badge">{node.kind}</span>
      <span class="status-badge">{node.status}</span>
    </div>
    <h2>{node.title}</h2>
    <code class="node-id">{node.id}</code>
  </header>

  <dl class="node-meta">
    <div>
      <dt>Lane</dt>
      <dd>{node.lane}</dd>
    </div>
    <div>
      <dt>Status</dt>
      <dd>{node.status}</dd>
    </div>
  </dl>

  <section class="completion-evidence" aria-label="Completion evidence">
    <h3>Completion evidence</h3>
    {#if provenancePresentation}
      <dl class="completion-evidence-grid">
        <div>
          <dt>Provenance</dt>
          <dd
            class:provenance-inferred={completionProvenance === 'inferred'}
            data-testid="completion-provenance"
          >{provenancePresentation.label}</dd>
        </div>
      </dl>
      <p class="provenance-detail">{provenancePresentation.detail}</p>
      <h4>Source references</h4>
      {#if completionSourceRefs.length > 0}
        <ul class="source-reference-list" aria-label="Completion source references">
          {#each completionSourceRefs as sourceRef}
            <li><code>{sourceRef}</code></li>
          {/each}
        </ul>
      {:else}
        <p class="source-reference-empty">No source references recorded</p>
      {/if}
    {:else}
      <p class="completion-evidence-empty">No completion provenance recorded</p>
    {/if}
  </section>

  <section class="contract" aria-label="Node contract">
    {#if hasContract}
      {#if node.contract.inputs.length > 0}
        <section>
          <h3>Inputs</h3>
          <ul>
            {#each node.contract.inputs as input}
              <li>{input}</li>
            {/each}
          </ul>
        </section>
      {/if}

      {#if node.contract.outputs.length > 0}
        <section>
          <h3>Outputs</h3>
          <ul>
            {#each node.contract.outputs as output}
              <li>{output}</li>
            {/each}
          </ul>
        </section>
      {/if}

      {#if node.contract.acceptance.length > 0}
        <section>
          <h3>Acceptance</h3>
          <ul>
            {#each node.contract.acceptance as criterion}
              <li>{criterion}</li>
            {/each}
          </ul>
        </section>
      {/if}
    {:else}
      <p class="contract-empty">No contract recorded</p>
    {/if}
  </section>

  <section class="progress" aria-label="Node progress">
    <h3>Progress</h3>
    {#if progress}
      <dl class="progress-grid">
        <div>
          <dt>Started</dt>
          <dd>{progress.started_at ?? 'Not recorded'}</dd>
        </div>
        <div>
          <dt>Finished</dt>
          <dd>{progress.finished_at ?? 'Not recorded'}</dd>
        </div>
        <div>
          <dt>Attempts</dt>
          <dd>{progress.attempts}</dd>
        </div>
        <div>
          <dt>Agent</dt>
          <dd>{progress.agent_id ?? 'Not recorded'}</dd>
        </div>
        <div>
          <dt>Last heartbeat</dt>
          <dd>{progress.last_heartbeat_at ?? 'Not recorded'}</dd>
        </div>
      </dl>
    {:else}
      <p class="progress-empty">No progress recorded</p>
    {/if}
  </section>

  <section class="dependencies">
    <h3>Immediate dependencies</h3>
    {#if dependencies.length > 0}
      <ul>
        {#each dependencies as dependency}
          <li>
            <span>{dependency.title}</span>
            <code>{dependency.id}</code>
            <span class="dependency-kind">{dependency.kind}</span>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="dependencies-empty">No immediate dependencies</p>
    {/if}
  </section>
</aside>

<style>
  .node-inspector {
    box-sizing: border-box;
    width: min(360px, 100%);
    padding: var(--space-4);
    border: 1px solid var(--border-structural);
    color: var(--text-primary);
    background-color: var(--bg-panel);
    box-shadow: var(--elev-2), var(--edge-lip);
  }

  .inspector-header {
    padding-bottom: var(--space-3);
    box-shadow: var(--edge-seam);
  }

  .badge-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }

  .kind-badge,
  .status-badge,
  .dependency-kind {
    border-radius: var(--radius-sm);
    font: var(--text-micro) var(--font-mono);
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  .kind-badge {
    padding: var(--space-1) var(--space-2);
    color: var(--accent-cyan);
    background-color: color-mix(in srgb, var(--accent-cyan) 8%, var(--bg-raised));
  }

  .status-badge {
    color: var(--text-secondary);
  }

  h2 {
    margin: var(--space-3) 0 var(--space-1);
    overflow-wrap: anywhere;
    font-family: var(--font-display);
    font-size: var(--text-h2);
    line-height: 1.15;
  }

  .node-id {
    color: var(--text-disabled);
    font: var(--text-micro) var(--font-mono);
    overflow-wrap: anywhere;
  }

  .node-meta,
  .progress-grid,
  .completion-evidence-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--space-2);
    margin: var(--space-3) 0 0;
  }

  .node-meta div,
  .progress-grid div,
  .completion-evidence-grid div {
    min-width: 0;
    padding: var(--space-2);
    border-radius: var(--radius-sm);
    background-color: var(--bg-raised);
  }

  dt,
  h3,
  h4 {
    color: var(--text-secondary);
    font: var(--text-micro) var(--font-mono);
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  dd {
    margin: var(--space-1) 0 0;
    overflow-wrap: anywhere;
    color: var(--text-primary);
    font: var(--text-small) var(--font-body);
  }

  .contract,
  .completion-evidence,
  .progress,
  .dependencies {
    margin-top: var(--space-4);
  }

  .contract section + section {
    margin-top: var(--space-3);
  }

  h3 {
    margin: 0 0 var(--space-2);
  }

  h4 {
    margin: var(--space-3) 0 var(--space-2);
  }

  .completion-evidence-grid {
    margin-top: 0;
  }

  .provenance-inferred {
    color: var(--status-success);
  }

  .provenance-detail,
  .completion-evidence-empty,
  .source-reference-empty {
    margin: var(--space-2) 0 0;
    color: var(--text-secondary);
    font: var(--text-small) var(--font-body);
  }

  .source-reference-list {
    gap: var(--space-1);
    padding-left: 0;
    list-style: none;
  }

  .source-reference-list li {
    min-width: 0;
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-sm);
    background-color: var(--bg-raised);
  }

  .source-reference-list code {
    overflow-wrap: anywhere;
    color: var(--text-primary);
    font: var(--text-micro) var(--font-mono);
  }

  ul {
    display: grid;
    gap: var(--space-1);
    margin: 0;
    padding-left: var(--space-4);
    color: var(--text-primary);
    font: var(--text-small) var(--font-body);
  }

  li::marker {
    color: var(--accent-cyan);
  }

  .contract-empty,
  .progress-empty,
  .dependencies-empty {
    margin: 0;
    color: var(--text-secondary);
    font: var(--text-small) var(--font-body);
  }

  .dependencies {
    padding-top: var(--space-3);
    box-shadow: var(--edge-seam-top);
  }

  .progress-grid div:last-child {
    grid-column: 1 / -1;
  }

  .dependencies li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: var(--space-1) var(--space-2);
    align-items: baseline;
  }

  .dependencies li code {
    color: var(--text-disabled);
    font: var(--text-micro) var(--font-mono);
  }

  .dependency-kind {
    grid-column: 1 / -1;
    color: var(--accent-chrome);
  }
</style>
