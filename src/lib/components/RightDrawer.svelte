<script lang="ts">
  import PlanView from './PlanView.svelte';
  import CoordinationPanel from './CoordinationPanel.svelte';

  type Tab = 'plan' | 'logs';
  let activeTab: Tab = $state('plan');
</script>

<div class="right-drawer">
  <div class="tab-bar">
    <button
      class="tab"
      class:active={activeTab === 'plan'}
      onclick={() => activeTab = 'plan'}
    >
      Plan
    </button>
    <button
      class="tab"
      class:active={activeTab === 'logs'}
      onclick={() => activeTab = 'logs'}
    >
      Logs
    </button>
  </div>

  <div class="tab-content">
    {#if activeTab === 'plan'}
      <PlanView />
    {:else}
      <CoordinationPanel />
    {/if}
  </div>
</div>

<style>
  .right-drawer {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-secondary, #1a1b26);
  }

  .tab-bar {
    display: flex;
    background: var(--bg-tertiary, #24283b);
    border-bottom: 1px solid var(--border-color, #414868);
    padding: 0 8px;
  }

  .tab {
    padding: 10px 16px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary, #565f89);
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    cursor: pointer;
    transition: color 0.15s, border-color 0.15s;
  }

  .tab:hover {
    color: var(--text-primary, #c0caf5);
  }

  .tab.active {
    color: var(--accent-color, #7aa2f7);
    border-bottom-color: var(--accent-color, #7aa2f7);
  }

  .tab-content {
    flex: 1;
    overflow: hidden;
  }
</style>
