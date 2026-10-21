<script lang="ts">
  import StatusPanel from '../StatusPanel.svelte';
  import PlanView from '../PlanView.svelte';
  import WorkGraphView from '../workgraph/WorkGraphView.svelte';

  let section = $state<'Status' | 'Plan' | 'Graph'>('Status');
</script>

<div class="overview-tab">
  <div class="sections" role="group" aria-label="Overview sections">
    {#each ['Status', 'Plan', 'Graph'] as label}
      <button type="button" class={`lattice-btn lattice-btn--compact ${section === label ? 'lattice-btn--primary' : 'lattice-btn--ghost'}`}
        aria-pressed={section === label} onclick={() => section = label as typeof section}>{label}</button>
    {/each}
  </div>
  <section class="section-content" aria-label={section}>
    {#if section === 'Status'}<StatusPanel />
    {:else if section === 'Plan'}<PlanView />
    {:else}<WorkGraphView />{/if}
  </section>
</div>

<style>
  .overview-tab { display: flex; flex: 1; min-height: 0; flex-direction: column; }
  .sections { display: flex; flex-wrap: wrap; gap: 4px; padding: 8px; }
  .section-content { display: flex; flex: 1; min-height: 0; flex-direction: column; overflow: auto; }
</style>
