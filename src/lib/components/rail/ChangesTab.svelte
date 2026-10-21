<script lang="ts">
  import { ui } from '$lib/stores/ui';
  import ReviewView from '../review/ReviewView.svelte';
  import SessionFilesView from '../SessionFilesView.svelte';
  import ArtifactBrowser from '../artifacts/ArtifactBrowser.svelte';

  let section = $state<'Review' | 'Files' | 'Artifacts'>('Review');
</script>

<div class="changes-tab">
  <div class="sections" role="group" aria-label="Changes sections">
    {#each ['Review', 'Files', 'Artifacts'] as label}
      <button type="button" class={`lattice-btn lattice-btn--compact ${section === label ? 'lattice-btn--primary' : 'lattice-btn--ghost'}`}
        aria-pressed={section === label} onclick={() => section = label as typeof section}>{label}</button>
    {/each}
  </div>
  <section class="section-content" aria-label={section}>
    {#if section === 'Review'}<ReviewView preferredAgentId={$ui.focusedAgentId ?? undefined} />
    {:else if section === 'Files'}<SessionFilesView />
    {:else}<ArtifactBrowser />{/if}
  </section>
</div>

<style>
  .changes-tab { display: flex; flex: 1; min-height: 0; flex-direction: column; }
  .sections { display: flex; flex-wrap: wrap; gap: 4px; padding: 8px; }
  .section-content { display: flex; flex: 1; min-height: 0; flex-direction: column; overflow: auto; }
</style>
