<script lang="ts">
  import { onMount } from 'svelte';
  import { check } from '@tauri-apps/plugin-updater';
  import { relaunch } from '@tauri-apps/plugin-process';

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

      await update.downloadAndInstall((event) => {
        if (event.event === 'Started' && event.data.contentLength) {
          progress = 0;
        } else if (event.event === 'Progress') {
          progress = Math.round((event.data.chunkLength / (event.data.contentLength || 1)) * 100);
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
  <div class="update-banner">
    <div class="update-content">
      <span class="update-icon">⬆</span>
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
        <button class="btn-update" on:click={downloadAndInstall}>
          Update Now
        </button>
        <button class="btn-dismiss" on:click={dismiss}>
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
    background: var(--bg-secondary, #1a1b26);
    border: 1px solid var(--accent-color, #7aa2f7);
    border-radius: 8px;
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 1000;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
    max-width: 300px;
  }

  .update-content {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .update-icon {
    font-size: 16px;
  }

  .update-text {
    font-size: 13px;
    color: var(--text-primary, #c0caf5);
  }

  .update-error {
    font-size: 11px;
    color: var(--color-error, #f7768e);
  }

  .update-actions {
    display: flex;
    gap: 8px;
  }

  .btn-update {
    padding: 6px 12px;
    font-size: 12px;
    background: var(--accent-color, #7aa2f7);
    border: none;
    border-radius: 4px;
    color: white;
    cursor: pointer;
    font-weight: 500;
  }

  .btn-update:hover {
    opacity: 0.9;
  }

  .btn-dismiss {
    padding: 6px 12px;
    font-size: 12px;
    background: transparent;
    border: 1px solid var(--border-color, #414868);
    border-radius: 4px;
    color: var(--text-secondary, #565f89);
    cursor: pointer;
  }

  .btn-dismiss:hover {
    background: var(--bg-tertiary, #24283b);
  }
</style>
