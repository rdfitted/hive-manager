<script lang="ts">
  import ConversationViewer from '../ConversationViewer.svelte';
  import CoordinationPanel from '../CoordinationPanel.svelte';
  import TimelineView from '../timeline/TimelineView.svelte';
  import ReplayView from '../replay/ReplayView.svelte';
  import ReplayControls from '../replay/ReplayControls.svelte';

  import type { ActivityTab } from '$lib/stores/layout';

  let { tab = 'activity' }: { tab?: ActivityTab } = $props();
  const section = $derived({ activity: 'Chat', logs: 'Logs', timeline: 'Timeline' }[tab]);
</script>

<div class="activity-tab">
  <section class="section-content" aria-label={section}>
    {#if section === 'Chat'}<ConversationViewer />
    {:else if section === 'Logs'}<CoordinationPanel />
    {:else}<TimelineView />{/if}
  </section>
  <details class="replay-section">
    <summary>Replay</summary>
    <div class="replay-content lattice-scroll-content"><ReplayView /></div>
    <ReplayControls />
  </details>
</div>

<style>
  .activity-tab { display: flex; flex: 1; min-height: 0; flex-direction: column; }
  .section-content { display: flex; flex: 1; min-height: 0; flex-direction: column; overflow: auto; }
  .replay-section { flex: none; max-height: 45%; overflow: auto; }
  summary { padding: 8px 12px; cursor: pointer; font-size: var(--text-small); }
  .replay-content { height: 160px; overflow: auto; }
</style>
