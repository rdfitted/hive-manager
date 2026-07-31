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
        class="lattice-btn lattice-btn--primary lattice-btn--compact"
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
    border-radius: var(--radius-lg);
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

  .no-session {
    color: var(--text-secondary);
    font-size: 12px;
    text-align: center;
    padding: 16px;
    font-style: italic;
  }
</style>
