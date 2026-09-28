<script lang="ts">
  import { onMount } from 'svelte';
  import { check, type Update } from '@tauri-apps/plugin-updater';
  import { relaunch } from '@tauri-apps/plugin-process';
  import { getVersion } from '@tauri-apps/api/app';
  import { invoke } from '@tauri-apps/api/core';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { ArrowClockwise, ArrowUp } from 'phosphor-svelte';
  import { sessionStateToCellStatus, type Session } from '$lib/stores/sessions';

  const UP_TO_DATE_MS = 4000;

  let currentVersion = '';
  let updateAvailable = false;
  let updateVersion = '';
  let downloading = false;
  let progress = 0;
  let error: string | null = null;
  let checking = false;
  let restartPending = false;
  // Only a check the operator asked for confirms "up to date"; background checks stay silent.
  let upToDate = false;
  let upToDateTimer: number | undefined;

  async function checkForUpdates(manual = false) {
    if (checking || downloading || restartPending) return;
    checking = true;
    error = null;
    upToDate = false;
    window.clearTimeout(upToDateTimer);
    try {
      const update = await check();
      try {
        updateAvailable = !!update;
        updateVersion = update?.version ?? '';
      } finally {
        await update?.close();
      }
      if (manual && !updateAvailable) {
        upToDate = true;
        upToDateTimer = window.setTimeout(() => (upToDate = false), UP_TO_DATE_MS);
      }
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
    getVersion()
      .then((version) => (currentVersion = version))
      .catch(() => {
        // Outside Tauri (plain browser dev) there is no app version to show.
      });
    void checkForUpdates();
    const interval = window.setInterval(() => void checkForUpdates(), 6 * 60 * 60 * 1000);
    return () => {
      window.clearInterval(interval);
      window.clearTimeout(upToDateTimer);
    };
  });

  async function confirmLiveSessions() {
    // Refresh from the backend so a session started in another window is included.
    const currentSessions = await invoke<Session[]>('list_sessions');
    const liveCount = currentSessions.filter((session) => {
      const status = sessionStateToCellStatus(session.state);
      return status !== 'completed' && status !== 'failed';
    }).length;
    if (liveCount === 0) return true;
    return confirm(`${liveCount} session${liveCount === 1 ? ' is' : 's are'} still active. Install the update now?`, {
      title: 'Hive Manager update', kind: 'warning', okLabel: 'Install update', cancelLabel: 'Later',
    });
  }

  async function restartWhenSafe(alreadyConfirmed = false) {
    try {
      if (!alreadyConfirmed && !await confirmLiveSessions()) return;
      await relaunch();
    } catch (e) {
      error = 'Could not restart the app. Please restart it manually.';
    }
  }

  async function downloadAndInstall() {
    downloading = true;
    error = null;
    let update: Update | null = null;
    let installCompleted = false;

    try {
      update = await check();
      if (!update) {
        updateAvailable = false;
        updateVersion = '';
        return;
      }
      if (!await confirmLiveSessions()) return;

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

      installCompleted = true;
      restartPending = true;
      updateAvailable = false;
      await restartWhenSafe(true);
    } catch (e) {
      error = 'Could not install the update. Try again later.';
    } finally {
      downloading = false;
      if (update && !installCompleted) {
        try {
          await update.close();
        } catch {
          error = 'Could not release the update check. Try again later.';
        }
      }
    }
  }

  function dismiss() {
    updateAvailable = false;
  }
</script>

{#if !updateAvailable || checking || restartPending || error}
<div class="update-check-actions" class:expanded={checking || restartPending || upToDate || !!error}>
  {#if !updateAvailable && !restartPending}
    <button class="check-icon" on:click={() => checkForUpdates(true)} disabled={checking || downloading} aria-label="Check for updates" title="Check for updates">
      <ArrowClockwise size={16} weight="light" aria-hidden="true" />
    </button>
  {/if}
  {#if currentVersion}<span class="app-version" title="Installed version">v{currentVersion}</span>{/if}
  {#if checking}<span class="update-status">Checking for updates...</span>{/if}
  {#if upToDate}<span class="update-status" role="status">Up to date</span>{/if}
  {#if restartPending}
    <button class="lattice-btn lattice-btn--primary" on:click={() => restartWhenSafe()}>Restart to finish update</button>
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
    {#if currentVersion && !downloading}
      <span class="update-installed">Installed: v{currentVersion}</span>
    {/if}

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
    max-width: min(280px, calc(100vw - 32px));
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
  /* Sits over terminal output when the row is not expanded, so it carries its own surface. */
  .app-version {
    display: inline-flex;
    align-items: center;
    height: 28px;
    padding: 0 8px;
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    background: var(--bg-panel);
    box-shadow: var(--elev-1);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
    white-space: nowrap;
  }
  .update-check-actions.expanded .app-version {
    border-color: transparent;
    background: transparent;
    box-shadow: none;
    padding: 0 2px;
  }
  .update-status {
    font-size: 11px;
    color: var(--text-secondary);
    white-space: nowrap;
  }
  .update-installed {
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
