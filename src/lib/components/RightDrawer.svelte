<script lang="ts">
  import PlanView from './PlanView.svelte';
  import CoordinationPanel from './CoordinationPanel.svelte';

  type Tab = 'plan' | 'logs';
  let activeTab: Tab = $state('plan');
  let collapsed = $state(true);
</script>

<div class="right-drawer" class:collapsed>
  <button class="drawer-header" onclick={() => collapsed = !collapsed} title={collapsed ? "Expand Panel" : "Collapse Panel"}>
    <span class="drawer-icon">📝</span>
    {#if !collapsed}
      <span class="drawer-title">Panel</span>
    {/if}
  </button>

  {#if !collapsed}
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
  {/if}
</div>

<style>
  .right-drawer {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
    background: var(--bg-secondary, #1a1b26);
  }

  .drawer-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 16px;
    border-bottom: 1px solid var(--border-color, #414868);
    background: none;
    border-left: none;
    border-right: none;
    border-top: none;
    cursor: pointer;
    width: 100%;
    text-align: left;
  }

  .drawer-header:hover {
    background: var(--bg-tertiary, #24283b);
  }

  .drawer-icon {
    font-size: 18px;
    flex-shrink: 0;
  }

  .drawer-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary, #c0caf5);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    white-space: nowrap;
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
