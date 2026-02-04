<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { currentBranch, availableBranches } from '$lib/stores/sessions';

  interface BranchInfo {
    name: string;
    short_hash: string;
    is_current: boolean;
  }

  let loading = $state(false);
  let pulling = $state(false);
  let error = $state<string | null>(null);

  async function loadBranches() {
    loading = true;
    error = null;
    try {
      const branches: BranchInfo[] = await invoke('list_branches');
      availableBranches.set(branches);
      const current = branches.find(b => b.is_current);
      if (current) {
        currentBranch.set(current.name);
      }
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function handleBranchChange(event: Event) {
    const target = event.target as HTMLSelectElement;
    const branch = target.value;
    
    loading = true;
    error = null;
    try {
      await invoke('switch_branch', { branch });
      await loadBranches();
    } catch (e) {
      error = String(e);
      // Revert select to current branch on error
      target.value = $currentBranch || '';
    } finally {
      loading = false;
    }
  }

  async function handlePull() {
    pulling = true;
    error = null;
    try {
      await invoke('git_pull');
      await loadBranches();
    } catch (e) {
      error = String(e);
    } finally {
      pulling = false;
    }
  }

  onMount(() => {
    loadBranches();
  });
</script>

<div class="branch-selector">
  <label>Branch:</label>
  {#if loading}
    <span class="loading">Loading...</span>
  {:else if error}
    <span class="error">{error}</span>
  {:else}
    <select value={$currentBranch} onchange={handleBranchChange}>
      {#each $availableBranches as branch}
        <option value={branch.name}>
          {branch.name} ({branch.short_hash})
        </option>
      {/each}
    </select>
    <button class="action-btn refresh-btn" onclick={loadBranches} title="Refresh branches" disabled={loading || pulling}>↻</button>
    <button class="action-btn pull-btn" onclick={handlePull} title="Pull from remote" disabled={loading || pulling}>
      {#if pulling}
        <span class="spinner">↻</span>
      {:else}
        ↓
      {/if}
    </button>
  {/if}
</div>

<style>
  .branch-selector {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .branch-selector label {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-primary, #c0caf5);
    white-space: nowrap;
  }

  .branch-selector select {
    flex: 1;
    max-width: 200px;
    padding: 4px 8px;
    font-size: 11px;
    background: var(--bg-tertiary, #24283b);
    border: 1px solid var(--border-color, #414868);
    border-radius: 4px;
    color: var(--text-primary, #c0caf5);
    cursor: pointer;
    /* Fix dropdown appearance */
    appearance: none;
    -webkit-appearance: none;
    -moz-appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12'%3E%3Cpath fill='%23565f89' d='M3 4.5L6 7.5L9 4.5'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 8px center;
    padding-right: 24px;
  }

  .branch-selector select:focus {
    outline: none;
    border-color: var(--accent-color, #7aa2f7);
  }

  .branch-selector select option {
    background: var(--bg-tertiary, #24283b);
    color: var(--text-primary, #c0caf5);
    padding: 8px;
  }

  .branch-selector select option:hover,
  .branch-selector select option:focus,
  .branch-selector select option:checked {
    background: var(--accent-color, #7aa2f7);
    color: white;
  }

  .action-btn {
    padding: 4px 8px;
    font-size: 14px;
    background: var(--bg-tertiary, #24283b);
    border: 1px solid var(--border-color, #414868);
    border-radius: 4px;
    color: var(--text-primary, #c0caf5);
    cursor: pointer;
    transition: all 0.2s;
    min-width: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .action-btn:hover:not(:disabled) {
    background: var(--bg-secondary, #1a1b26);
    border-color: var(--accent-color, #7aa2f7);
    color: var(--accent-color, #7aa2f7);
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .pull-btn:hover:not(:disabled) {
    border-color: var(--color-success, #9ece6a);
    color: var(--color-success, #9ece6a);
  }

  .spinner {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }

  .loading {
    font-size: 11px;
    color: var(--text-secondary, #565f89);
  }

  .error {
    font-size: 11px;
    color: var(--color-error, #f7768e);
  }
</style>
