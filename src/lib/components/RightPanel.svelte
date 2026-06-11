<script lang="ts">
  import { CaretLeft, CaretRight, ChartBar, ChatCenteredText, ClockCounterClockwise, ListBullets, NotePencil } from 'phosphor-svelte';
  import { layout, RAIL_WIDTH, type RightPanelTab } from '$lib/stores/layout';
  import StatusPanel from './StatusPanel.svelte';
  import PlanView from './PlanView.svelte';
  import CoordinationPanel from './CoordinationPanel.svelte';
  import ConversationViewer from './ConversationViewer.svelte';
  import TimelineView from './timeline/TimelineView.svelte';
  import ResizeHandle from './ResizeHandle.svelte';

  const TABS: Array<{ id: RightPanelTab; label: string; icon: typeof ChartBar }> = [
    { id: 'status', label: 'Status', icon: ChartBar },
    { id: 'plan', label: 'Plan', icon: NotePencil },
    { id: 'logs', label: 'Logs', icon: ListBullets },
    { id: 'chat', label: 'Chat', icon: ChatCenteredText },
    { id: 'timeline', label: 'Timeline', icon: ClockCounterClockwise },
  ];

  let resizing = $state(false);
  let collapsed = $derived($layout.rightCollapsed);
  let activeTab = $derived($layout.rightTab);
  let panelWidth = $derived(collapsed ? RAIL_WIDTH : $layout.rightWidth);

  function handleResize(clientX: number) {
    layout.setRightWidth(window.innerWidth - clientX);
  }
</script>

<aside
  class="right-panel"
  class:collapsed
  class:resizing
  style:width={`${panelWidth}px`}
  style:min-width={`${panelWidth}px`}
>
  {#if collapsed}
    <div class="rail">
      <button
        type="button"
        class="rail-button expand"
        onclick={() => layout.toggleRight()}
        title="Expand panel (Ctrl+J)"
        aria-label="Expand panel"
      >
        <CaretLeft size={14} weight="light" />
      </button>
      {#each TABS as tab (tab.id)}
        <button
          type="button"
          class="rail-button"
          class:active={activeTab === tab.id}
          onclick={() => layout.setRightTab(tab.id)}
          title={tab.label}
          aria-label={tab.label}
        >
          <tab.icon size={18} weight="light" />
        </button>
      {/each}
    </div>
  {:else}
    <div class="panel-header">
      <nav class="tab-bar" aria-label="Panel tabs">
        {#each TABS as tab (tab.id)}
          <button
            type="button"
            class="tab"
            class:active={activeTab === tab.id}
            onclick={() => layout.setRightTab(tab.id)}
            title={tab.label}
          >
            <tab.icon size={15} weight="light" />
            <span class="tab-label">{tab.label}</span>
          </button>
        {/each}
      </nav>
      <button
        type="button"
        class="collapse-chevron"
        onclick={() => layout.toggleRight()}
        title="Collapse panel (Ctrl+J)"
        aria-label="Collapse panel"
      >
        <CaretRight size={14} weight="light" />
      </button>
    </div>

    <div class="tab-content">
      {#if activeTab === 'status'}
        <StatusPanel />
      {:else if activeTab === 'plan'}
        <PlanView />
      {:else if activeTab === 'logs'}
        <CoordinationPanel />
      {:else if activeTab === 'timeline'}
        <TimelineView />
      {:else}
        <ConversationViewer />
      {/if}
    </div>

    <ResizeHandle
      label="Resize panel"
      onResize={handleResize}
      onDragChange={(d) => resizing = d}
    />
  {/if}
</aside>

<style>
  .right-panel {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg-surface);
    border-left: 1px solid var(--border-structural);
    transition: width 0.2s ease, min-width 0.2s ease;
  }

  .right-panel.resizing {
    transition: none;
  }

  .right-panel :global(.resize-handle) {
    left: -3px;
  }

  .rail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 8px 0;
  }

  .rail-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    cursor: pointer;
    transition: color 0.15s ease, background 0.15s ease, border-color 0.15s ease;
  }

  .rail-button:hover {
    color: var(--accent-cyan);
    background: var(--bg-elevated);
  }

  .rail-button.active {
    color: var(--accent-cyan);
    background: var(--bg-elevated);
    border-color: var(--border-structural);
  }

  .rail-button.expand {
    margin-bottom: 6px;
    border-bottom: 1px solid var(--border-structural);
    border-radius: 0;
    width: 100%;
    padding-bottom: 12px;
  }

  .panel-header {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 6px;
    border-bottom: 1px solid var(--border-structural);
  }

  .tab-bar {
    display: flex;
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }

  .tab {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    cursor: pointer;
    white-space: nowrap;
    transition: color 0.15s, border-color 0.15s;
  }

  .tab:hover {
    color: var(--text-primary);
  }

  .tab.active {
    color: var(--accent-cyan);
    border-bottom-color: var(--accent-cyan);
  }

  .tab-label {
    display: inline;
  }

  .collapse-chevron {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    border-radius: var(--radius-sm);
    flex-shrink: 0;
  }

  .collapse-chevron:hover {
    background: var(--bg-elevated);
    color: var(--accent-cyan);
  }

  .tab-content {
    flex: 1;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
</style>
