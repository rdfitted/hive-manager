<script lang="ts">
  import { ui } from '$lib/stores/ui';
  import ReviewView from '../review/ReviewView.svelte';
  import SessionFilesView from '../SessionFilesView.svelte';
  import ArtifactBrowser from '../artifacts/ArtifactBrowser.svelte';

  import type { ChangesTab } from '$lib/stores/layout';

  let { tab = 'changes' }: { tab?: ChangesTab } = $props();
  const section = $derived({ changes: 'Review', files: 'Files', artifacts: 'Artifacts' }[tab]);
</script>

<div class="changes-tab">
  <section class="section-content" aria-label={section}>
    {#if section === 'Review'}<ReviewView preferredAgentId={$ui.focusedAgentId ?? undefined} />
    {:else if section === 'Files'}<SessionFilesView />
    {:else}<ArtifactBrowser />{/if}
  </section>
</div>

<style>
  .changes-tab { display: flex; flex: 1; min-height: 0; flex-direction: column; }
  .section-content { display: flex; flex: 1; min-height: 0; flex-direction: column; overflow: auto; }
</style>
