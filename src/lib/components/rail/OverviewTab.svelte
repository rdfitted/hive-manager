<script lang="ts">
  import StatusPanel from '../StatusPanel.svelte';
  import PlanView from '../PlanView.svelte';
  import WorkGraphView from '../workgraph/WorkGraphView.svelte';

  import type { OverviewTab } from '$lib/stores/layout';

  let { tab = 'overview' }: { tab?: OverviewTab } = $props();
  const section = $derived({ overview: 'Status', plan: 'Plan', graph: 'Graph' }[tab]);
</script>

<div class="overview-tab">
  <section class="section-content" aria-label={section}>
    {#if section === 'Status'}<StatusPanel />
    {:else if section === 'Plan'}<PlanView />
    {:else}<WorkGraphView />{/if}
  </section>
</div>

<style>
  .overview-tab { display: flex; flex: 1; min-height: 0; flex-direction: column; }
  .section-content { display: flex; flex: 1; min-height: 0; flex-direction: column; overflow: auto; }
</style>
