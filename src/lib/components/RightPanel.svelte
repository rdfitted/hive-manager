<script lang="ts">
  import { CaretLeft, CaretRight, ChartBar, ChatCenteredText, ClockCounterClockwise, FileText, ListBullets, NotePencil } from 'phosphor-svelte';
  import { layout, RAIL_WIDTH, type RightPanelTab } from '$lib/stores/layout';
  import StatusPanel from './StatusPanel.svelte';
  import PlanView from './PlanView.svelte';
  import CoordinationPanel from './CoordinationPanel.svelte';
  import ConversationViewer from './ConversationViewer.svelte';
  import TimelineView from './timeline/TimelineView.svelte';
  import SessionFilesView from './SessionFilesView.svelte';
  import ResizeHandle from './ResizeHandle.svelte';

  const TABS: Array<{ id: RightPanelTab; label: string; icon: typeof ChartBar }> = [
    { id: 'status', label: 'Status', icon: ChartBar },
    { id: 'plan', label: 'Plan', icon: NotePencil },
    { id: 'files', label: 'Files', icon: FileText },
    { id: 'logs', label: 'Logs', icon: ListBullets },
    { id: 'chat', label: 'Chat', icon: ChatCenteredText },
    { id: 'timeline', label: 'Timeline', icon: ClockCounterClockwise },
  ];

  let collapsed = $derived($layout.rightCollapsed);
  let activeTab = $derived($layout.rightTab);
  let drawerWidth = $derived($layout.rightWidth - RAIL_WIDTH);

  function handleResize(clientX: number) {
    layout.setRightWidth(window.innerWidth - clientX);
  }
</script>

<aside
  class="right-panel"
  style:width={`${RAIL_WIDTH}px`}
  style:min-width={`${RAIL_WIDTH}px`}
  style:flex-basis={`${RAIL_WIDTH}px`}
>
  <div class="rail">
    <button
      type="button"
      class="rail-control expand lattice-btn lattice-btn--ghost lattice-btn--icon"
      onclick={() => layout.toggleRight()}
      title={collapsed ? "Expand panel (Ctrl+J)" : "Collapse panel (Ctrl+J)"}
      aria-label={collapsed ? "Expand panel" : "Collapse panel"}
    >
      {#if collapsed}
        <CaretLeft size={14} weight="light" />
      {:else}
        <CaretRight size={14} weight="light" />
      {/if}
    </button>
    {#each TABS as tab (tab.id)}
      {@const Icon = tab.icon}
      <button
        type="button"
        class={`rail-control lattice-btn lattice-btn--icon ${activeTab === tab.id ? 'lattice-btn--primary' : 'lattice-btn--ghost'}`}
        onclick={() => layout.setRightTab(tab.id)}
        title={tab.label}
        aria-label={tab.label}
      >
        <Icon size={18} weight="light" />
      </button>
    {/each}
  </div>

  <div
    class="panel-drawer"
    class:collapsed
    style:width={`${drawerWidth}px`}
    inert={collapsed}
    aria-hidden={collapsed}
  >
    <div class="panel-header">
      <h2 class="panel-title">{TABS.find((tab) => tab.id === activeTab)?.label}</h2>
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
      {:else if activeTab === 'files'}
        <SessionFilesView />
      {:else}
        <ConversationViewer />
      {/if}
    </div>

    <ResizeHandle
      label="Resize panel"
      onResize={handleResize}
    />
  </div>
</aside>

<style>
  .right-panel {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
    overflow: visible;
    /* Shell chrome intentionally remains outside the content-card surface primitive. */
    background: var(--bg-void);
    box-shadow: inset 1px 0 0 rgba(0, 0, 0, 0.45);
    z-index: 20;
  }

  .panel-drawer :global(.resize-handle) {
    left: -3px;
  }

  .rail {
    position: relative;
    z-index: 2;
    display: flex;
    flex: 1;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 8px 0;
    background: var(--bg-void);
  }

  .rail-control {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    padding: 0;
  }

  .rail-control.expand {
    margin-bottom: 6px;
    border-radius: var(--radius-none);
    width: 100%;
    padding-bottom: 12px;
    box-shadow: var(--edge-seam);
  }

  .panel-drawer {
    position: absolute;
    top: 11px;
    right: 100%;
    bottom: 11px;
    z-index: 1;
    display: flex;
    min-width: 0;
    flex-direction: column;
    overflow: hidden;
    border-radius: var(--radius-lg);
    background: var(--bg-panel);
    box-shadow: var(--elev-3), var(--edge-lip);
    transform: translateX(0);
    transition: transform var(--motion-duration-standard) var(--motion-ease-standard);
  }

  .panel-drawer.collapsed {
    pointer-events: none;
    transform: translateX(104%);
  }

  .panel-header {
    display: flex;
    align-items: center;
    min-height: 40px;
    padding: 4px 10px;
    box-shadow: var(--edge-seam);
  }

  .panel-title {
    margin: 0;
    font-family: var(--font-display);
    font-size: var(--text-h3);
    font-weight: 600;
  }

  .tab-content {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
</style>
