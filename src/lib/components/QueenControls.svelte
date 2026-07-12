<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { activeSession, serdeEnumVariantName } from '$lib/stores/sessions';
  import BranchSelector from './BranchSelector.svelte';

  const dispatch = createEventDispatcher<{
    openAddWorker: void;
  }>();

  $: activeSessionType = $activeSession
    ? serdeEnumVariantName($activeSession.session_type)
    : null;
  $: canAddPrincipal = !$activeSession?.no_git
    && (activeSessionType === 'Hive' || activeSessionType === 'Swarm');
</script>

<div class="queen-controls">
  <div class="controls-header">
    <h4>Session Controls</h4>
    {#if canAddPrincipal}
      <button
        type="button"
        class="add-worker-btn"
        on:click={() => dispatch('openAddWorker')}
        title="Add managed principal"
      >
        + Add Principal
      </button>
    {/if}
  </div>

  {#if !$activeSession}
    <div class="no-session">No active session</div>
  {:else if $activeSession.no_git}
    <div class="no-session">Research session: git controls are intentionally disabled.</div>
  {:else}
    <div class="branch-section">
      <BranchSelector />
    </div>
  {/if}
</div>

<style>
  .queen-controls {
    padding: 12px;
    background: var(--bg-void);
    border-radius: var(--radius-sm);
  }

  .controls-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 12px;
  }

  .controls-header h4 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .add-worker-btn {
    padding: 4px 10px;
    font-size: 11px;
    background: var(--accent-cyan);
    border: none;
    border-radius: var(--radius-sm);
    color: white;
    cursor: pointer;
    font-weight: 500;
  }

  .add-worker-btn:hover {
    opacity: 0.9;
  }

  .no-session {
    color: var(--text-secondary);
    font-size: 12px;
    text-align: center;
    padding: 16px;
    font-style: italic;
  }
</style>
