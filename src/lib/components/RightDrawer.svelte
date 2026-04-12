<script lang="ts">
  import { NotePencil } from 'phosphor-svelte';
  import PlanView from './PlanView.svelte';
  import CoordinationPanel from './CoordinationPanel.svelte';
  import ConversationViewer from './ConversationViewer.svelte';
  import TimelineView from './timeline/TimelineView.svelte';

  type Tab = 'plan' | 'logs' | 'chat' | 'timeline';
  let activeTab: Tab = $state('plan');
  let collapsed = $state(true);
</script>

<div class="right-drawer" class:collapsed>
  <button class="drawer-header" onclick={() => collapsed = !collapsed} title={collapsed ? "Expand Panel" : "Collapse Panel"}>
    <span class="drawer-icon">
      <NotePencil size={18} weight="light" />
    </span>
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
      <button
        class="tab"
        class:active={activeTab === 'chat'}
        onclick={() => activeTab = 'chat'}
      >
        Chat
      </button>
      <button
        class="tab"
        class:active={activeTab === 'timeline'}
        onclick={() => activeTab = 'timeline'}
      >
        Timeline
      </button>
    </div>

    <div class="tab-content">
      {#if activeTab === 'plan'}
        <PlanView />
      {:else if activeTab === 'logs'}
        <CoordinationPanel />
      {:else if activeTab === 'timeline'}
        <TimelineView />
      {:else}
        <ConversationViewer />
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
    background: var(--bg-void);
  }

  .drawer-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 16px;
    border-bottom: 1px solid var(--border-structural);
    background: none;
    border-left: none;
    border-right: none;
    border-top: none;
    cursor: pointer;
    width: 100%;
    text-align: left;
  }

  .drawer-header:hover {
    background: var(--bg-surface);
  }

  .drawer-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .drawer-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    white-space: nowrap;
  }

  .tab-bar {
    display: flex;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border-structural);
    padding: 0 8px;
  }

  .tab {
    padding: 10px 16px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    cursor: pointer;
    transition: color 0.15s, border-color 0.15s;
  }

  .tab:hover {
    color: var(--text-primary);
  }

  .tab.active {
    color: var(--accent-cyan);
    border-bottom-color: var(--accent-cyan);
  }

  .tab-content {
    flex: 1;
    overflow: hidden;
  }
</style>
