<script lang="ts">
  import { onMount } from 'svelte';
  import { check } from '@tauri-apps/plugin-updater';
  import { relaunch } from '@tauri-apps/plugin-process';
  import { invoke } from '@tauri-apps/api/core';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { ArrowClockwise, ArrowUp } from 'phosphor-svelte';
  import { sessionStateToCellStatus, type Session } from '$lib/stores/sessions';

  let updateAvailable = false;
  let updateVersion = '';
  let downloading = false;
  let progress = 0;
  let error: string | null = null;
  let checking = false;
  let restartPending = false;

  async function checkForUpdates() {
    if (checking || downloading || restartPending) return;
    checking = true;
    error = null;
    try {
      const update = await check();
      updateAvailable = !!update;
      updateVersion = update?.version ?? '';
    } catch (e) {
      if (import.meta.env.DEV) {
        console.log('Update check skipped:', e);
      } else {
        error = 'Could not check for updates. Try again later.';
      }
    } finally {
      checking = false;
    }
  }

  onMount(() => {
    void checkForUpdates();
    const interval = window.setInterval(() => void checkForUpdates(), 6 * 60 * 60 * 1000);
    return () => window.clearInterval(interval);
  });

  async function restartWhenSafe() {
    try {
      // Refresh from the backend so a session started in another window is included.
      const currentSessions = await invoke<Session[]>('list_sessions');
      const liveCount = currentSessions.filter((session) => {
        const status = sessionStateToCellStatus(session.state);
        return status !== 'completed' && status !== 'failed';
      }).length;
      if (liveCount > 0 && !await confirm(`${liveCount} session${liveCount === 1 ? ' is' : 's are'} still active. Restart to finish updating?`, {
        title: 'Hive Manager update', kind: 'warning', okLabel: 'Restart', cancelLabel: 'Later',
      })) {
        return;
      }
      await relaunch();
    } catch (e) {
      error = 'Could not restart the app. Please restart it manually.';
    }
  }

  async function downloadAndInstall() {
    downloading = true;
    error = null;

    try {
      const update = await check();
      if (!update) {
        updateAvailable = false;
        updateVersion = '';
        return;
      }

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

      restartPending = true;
      updateAvailable = false;
      await restartWhenSafe();
    } catch (e) {
      error = 'Could not install the update. Try again later.';
    } finally {
      downloading = false;
    }
  }

  function dismiss() {
    updateAvailable = false;
  }
</script>

{#if !updateAvailable || checking || restartPending || error}
<div class="update-check-actions" class:expanded={checking || restartPending || !!error}>
  {#if !updateAvailable && !restartPending}
    <button class="check-icon" on:click={checkForUpdates} disabled={checking || downloading} aria-label="Check for updates" title="Check for updates">
      <ArrowClockwise size={16} weight="light" aria-hidden="true" />
    </button>
  {/if}
  {#if checking}<span class="update-status">Checking for updates...</span>{/if}
  {#if restartPending}
    <button class="lattice-btn lattice-btn--primary" on:click={restartWhenSafe}>Restart to finish update</button>
  {/if}
  {#if error && !updateAvailable}<span class="update-error" role="alert">{error}</span>{/if}
</div>
{/if}

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
      <span class="update-error" role="alert">{error}</span>
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
  .update-check-actions {
    position: fixed;
    bottom: 8px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 1000;
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: min(160px, calc(100vw - 32px));
  }
  .update-check-actions.expanded {
    padding: 6px;
    border-radius: var(--radius-sm);
    background: var(--bg-panel);
    box-shadow: var(--elev-2), var(--edge-lip);
  }
  .check-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    background: var(--bg-panel);
    color: var(--text-secondary);
    box-shadow: var(--elev-1);
  }
  .check-icon:hover:where(:not(:disabled)) {
    color: var(--text-primary);
    background: var(--bg-raised);
  }
  .update-status {
    font-size: 11px;
    color: var(--text-secondary);
  }
  .update-banner {
    position: fixed;
    bottom: 16px;
    right: 16px;
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 1000;
    box-shadow: var(--elev-3), var(--edge-lip);
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
