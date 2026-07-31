<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { currentBranch, availableBranches, activeSession } from '$lib/stores/sessions';
  import Skeleton from './Skeleton.svelte';
  import SkelBar from './SkelBar.svelte';

  interface BranchInfo {
    name: string;
    short_hash: string;
    is_current: boolean;
  }

  let loading = $state(false);
  let pulling = $state(false);
  let error = $state<string | null>(null);

  // Managed sessions execute in their primary worktree, not the base checkout.
  // Keep that branch read-only here so session controls cannot accidentally
  // switch the worktree out from under running agents.
  let managedWorkspace = $derived(Boolean($activeSession?.worktree_path));
  let projectPath = $derived($activeSession?.worktree_path ?? $activeSession?.project_path);
  let managedBranch = $derived($activeSession?.worktree_branch ?? 'Managed worktree');

  // Reload branches when project path changes
  $effect(() => {
    if (projectPath) {
      if (managedWorkspace) {
        availableBranches.set([]);
        currentBranch.set(managedBranch);
        error = null;
      } else {
        loadBranches();
      }
    } else {
      // Clear branches when no session
      availableBranches.set([]);
      currentBranch.set('');
    }
  });

  async function loadBranches() {
    if (!projectPath) return;

    loading = true;
    error = null;
    try {
      const branches: BranchInfo[] = await invoke('list_branches', { projectPath });
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
    if (!projectPath) return;

    const target = event.target as HTMLSelectElement;
    const branch = target.value;

    loading = true;
    error = null;
    try {
      await invoke('switch_branch', { projectPath, branch });
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
    if (!projectPath) return;

    pulling = true;
    error = null;
    try {
      await invoke('git_pull', { projectPath });
      await loadBranches();
    } catch (e) {
      error = String(e);
    } finally {
      pulling = false;
    }
  }
</script>

{#snippet branchSkeleton()}
  <div class="branch-skeleton">
    <SkelBar width="7rem" height="1.5rem" radius="md" />
  </div>
{/snippet}

<div class="branch-selector">
  <label for="branch-select">Branch:</label>
  <Skeleton loading={loading} skeleton={branchSkeleton} layout="inline" class="branch-loading">
    {#if error}
      <span class="error" title={error}>{error.slice(0, 30)}{error.length > 30 ? '...' : ''}</span>
      <button class="lattice-btn lattice-btn--ghost lattice-btn--icon" onclick={loadBranches} title="Retry">↻</button>
    {:else if !projectPath}
      <span class="branch-state">No session</span>
    {:else if managedWorkspace}
      <span class="managed-branch" title={projectPath}>{managedBranch}</span>
    {:else}
      <select class="lattice-input" id="branch-select" value={$currentBranch} onchange={handleBranchChange}>
        {#each $availableBranches as branch}
          <option value={branch.name}>
            {branch.name} ({branch.short_hash})
          </option>
        {/each}
      </select>
      <button class="lattice-btn lattice-btn--ghost lattice-btn--icon" onclick={loadBranches} title="Refresh branches" disabled={loading || pulling}>↻</button>
      <button class="lattice-btn lattice-btn--secondary lattice-btn--compact lattice-btn--icon" class:lattice-btn--waiting={pulling} aria-busy={pulling} onclick={handlePull} title="Pull from remote" disabled={loading || pulling}>
        {#if pulling}
          <!-- Pull feedback is an action state, so it keeps the shared spinner. -->
          <span class="lattice-motion-spinner">↻</span>
        {:else}
          ↓
        {/if}
      </button>
    {/if}
  </Skeleton>
</div>

<style>
  .branch-selector {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .branch-selector label {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-primary);
    white-space: nowrap;
  }

  .branch-skeleton {
    display: flex;
    align-items: center;
  }

  .branch-selector :global(.branch-loading) {
    flex: 1;
  }

  .branch-selector :global(.branch-loading .lattice-skeleton-content) {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .branch-selector select {
    flex: 1;
    min-width: 120px;
    max-width: 200px;
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

  /* Native option popovers cannot inherit the select control's shared surface. */
  .branch-selector select option {
    background: var(--bg-surface);
    color: var(--text-primary);
    padding: 8px;
  }

  .branch-selector select option:hover,
  .branch-selector select option:focus,
  .branch-selector select option:checked {
    background: var(--accent-cyan);
    color: white;
  }

  .branch-state {
    font-size: 11px;
    color: var(--text-secondary);
  }

  .managed-branch {
    font-size: 11px;
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .error {
    font-size: 11px;
    color: var(--status-error);
    max-width: 150px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
