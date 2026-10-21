<script lang="ts">
  import ConversationViewer from '../ConversationViewer.svelte';
  import CoordinationPanel from '../CoordinationPanel.svelte';
  import TimelineView from '../timeline/TimelineView.svelte';
  import ReplayView from '../replay/ReplayView.svelte';
  import ReplayControls from '../replay/ReplayControls.svelte';

  let section = $state<'Chat' | 'Logs' | 'Timeline'>('Chat');
</script>

<div class="activity-tab">
  <div class="sections" role="group" aria-label="Activity sections">
    {#each ['Chat', 'Logs', 'Timeline'] as label}
      <button type="button" class={`lattice-btn lattice-btn--compact ${section === label ? 'lattice-btn--primary' : 'lattice-btn--ghost'}`}
        aria-pressed={section === label} onclick={() => section = label as typeof section}>{label}</button>
    {/each}
  </div>
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
  .sections { display: flex; flex-wrap: wrap; gap: 4px; padding: 8px; }
  .section-content { display: flex; flex: 1; min-height: 0; flex-direction: column; overflow: auto; }
  .replay-section { flex: none; max-height: 45%; overflow: auto; }
  summary { padding: 8px 12px; cursor: pointer; font-size: var(--text-small); }
  .replay-content { height: 160px; overflow: auto; }
</style>
