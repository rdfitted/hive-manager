<script lang="ts">
  import { onMount } from 'svelte';
  import { check } from '@tauri-apps/plugin-updater';
  import { relaunch } from '@tauri-apps/plugin-process';
  import { ArrowUp } from 'phosphor-svelte';

  let updateAvailable = false;
  let updateVersion = '';
  let downloading = false;
  let progress = 0;
  let error: string | null = null;

  onMount(async () => {
    try {
      const update = await check();
      if (update) {
        updateAvailable = true;
        updateVersion = update.version;
      }
    } catch (e) {
      // Silently fail - updates not available in dev mode
      console.log('Update check skipped:', e);
    }
  });

  async function downloadAndInstall() {
    downloading = true;
    error = null;

    try {
      const update = await check();
      if (!update) return;

      let totalBytes = 0;
      let downloadedBytes = 0;

      await update.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          totalBytes = event.data.contentLength ?? 0;
          downloadedBytes = 0;
          progress = 0;
        } else if (event.event === 'Progress') {
          downloadedBytes += event.data.chunkLength;
          progress = totalBytes > 0 ? Math.round((downloadedBytes / totalBytes) * 100) : 0;
        } else if (event.event === 'Finished') {
          progress = 100;
        }
      });

      // Relaunch the app to apply the update
      await relaunch();
    } catch (e) {
      error = String(e);
      downloading = false;
    }
  }

  function dismiss() {
    updateAvailable = false;
  }
</script>

{#if updateAvailable}
  <div class="update-banner lattice-panel">
    <div class="update-content">
      <ArrowUp size={16} weight="light" />
      <span class="update-text">
        {#if downloading}
          Downloading update... {progress}%
        {:else}
          Update available: v{updateVersion}
        {/if}
      </span>
    </div>

    {#if error}
      <span class="update-error">{error}</span>
    {/if}

    <div class="update-actions">
      {#if !downloading}
        <button class="lattice-btn lattice-btn--primary" on:click={downloadAndInstall}>
          Update Now
        </button>
        <button class="lattice-btn lattice-btn--secondary" on:click={dismiss}>
          Later
        </button>
      {/if}
    </div>
  </div>
{/if}

<style>
  .update-banner {
    position: fixed;
    bottom: 16px;
    right: 16px;
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 1000;
    box-shadow: 0 4px 12px color-mix(in srgb, var(--bg-void) 70%, transparent);
    max-width: 300px;
  }

  .update-content {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .update-text {
    font-size: 13px;
    color: var(--text-primary);
  }

  .update-error {
    font-size: 11px;
    color: var(--status-error);
  }

  .update-actions {
    display: flex;
    gap: 8px;
  }

</style>
