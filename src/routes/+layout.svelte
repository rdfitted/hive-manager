<script lang="ts">
  import type { Snippet } from 'svelte';
  import '../lib/styles/lattice-tokens.css';
  import '../lib/styles/lattice.css';
  import SessionSidebar from '$lib/components/SessionSidebar.svelte';
  import AddWorkerDialog from '$lib/components/AddWorkerDialog.svelte';
  import {
    sessions,
    type DebateLaunchConfig,
    type FusionLaunchConfig,
    type HiveLaunchConfig,
  } from '$lib/stores/sessions';
  import { layout } from '$lib/stores/layout';
  import { shell } from '$lib/stores/shell';

  let { children }: { children?: Snippet } = $props();
  let showAddWorkerDialog = $state(false);

  $effect(() => {
    showAddWorkerDialog = $shell.addWorkerOpen;
  });

  async function handleLaunchHiveV2(config: HiveLaunchConfig): Promise<void> {
    await sessions.launchHiveV2(config);
  }

  async function handleLaunchFusion(config: FusionLaunchConfig): Promise<void> {
    await sessions.launchFusion(config);
  }

  async function handleLaunchDebate(config: DebateLaunchConfig): Promise<void> {
    await sessions.launchDebate(config);
  }

  function handleKeydown(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && event.key === 'b') {
      event.preventDefault();
      layout.toggleLeft();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="app">
  <SessionSidebar
    onLaunchHiveV2={handleLaunchHiveV2}
    onLaunchFusion={handleLaunchFusion}
    onLaunchDebate={handleLaunchDebate}
    onOpenAddWorker={() => shell.openAddWorker()}
    startAction={$shell.startAction}
  />
  {@render children?.()}
</div>

<AddWorkerDialog bind:open={showAddWorkerDialog} on:close={() => shell.closeAddWorker()} />
