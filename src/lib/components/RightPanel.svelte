<script lang="ts">
  import { ChartBar, ChatCenteredText, FileText, SidebarSimple } from 'phosphor-svelte';
  import { layout, RAIL_WIDTH, type RightPanelTab } from '$lib/stores/layout';
  import { shortcutLabel } from '$lib/platform';
  import OverviewTab from './rail/OverviewTab.svelte';
  import ActivityTab from './rail/ActivityTab.svelte';
  import ChangesTab from './rail/ChangesTab.svelte';
  import ResizeHandle from './ResizeHandle.svelte';

  const TABS: Array<{ id: RightPanelTab; label: string; icon: typeof ChartBar }> = [
    { id: 'overview', label: 'Overview', icon: ChartBar },
    { id: 'activity', label: 'Activity', icon: ChatCenteredText },
    { id: 'changes', label: 'Changes', icon: FileText },
  ];

  let collapsed = $derived($layout.rightCollapsed);
  const toggleShortcut = shortcutLabel('J');
  let toggleLabel = $derived(`${collapsed ? 'Expand' : 'Collapse'} panel (${toggleShortcut})`);
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
    {#each TABS as tab (tab.id)}
      {@const Icon = tab.icon}
      <button
        type="button"
        class={`rail-control lattice-btn lattice-btn--icon ${activeTab === tab.id ? 'lattice-btn--primary' : 'lattice-btn--ghost'}`}
        onclick={() => layout.activateRightTab(tab.id)}
        title={tab.label}
        aria-label={tab.label}
        aria-pressed={activeTab === tab.id && !collapsed}
      >
        <Icon size={18} weight="light" />
      </button>
    {/each}
    <!-- Pointer shortcut only; the labelled button below is the accessible control. -->
    <button
      type="button"
      class="rail-filler"
      tabindex="-1"
      aria-hidden="true"
      onclick={() => layout.toggleRight()}
      title={toggleLabel}
    ></button>
    <button
      type="button"
      class="rail-control rail-toggle lattice-btn lattice-btn--ghost lattice-btn--icon"
      onclick={() => layout.toggleRight()}
      title={toggleLabel}
      aria-label={toggleLabel}
      aria-expanded={!collapsed}
    >
      <!-- Mirrored so the highlighted pane sits on the right, next to this rail. -->
      <SidebarSimple size={18} weight="light" aria-hidden="true" class="mirrored" />
    </button>
  </div>

  <div
    class="panel-drawer lattice-forced-colors-boundary"
    class:collapsed
    style:width={`${drawerWidth}px`}
    inert={collapsed}
    aria-hidden={collapsed}
  >
    <div class="panel-header">
      <h2 class="panel-title">{TABS.find((tab) => tab.id === activeTab)?.label}</h2>
    </div>

    <div class="tab-content">
      {#if activeTab === 'overview'}
        <OverviewTab />
      {:else if activeTab === 'activity'}
        <ActivityTab />
      {:else}
        <ChangesTab />
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

  .rail-filler {
    align-self: stretch;
    flex: 1;
    min-height: 24px;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: pointer;
    transition: background-color var(--motion-duration-standard) var(--motion-ease-standard);
  }

  .rail-toggle {
    flex: none;
  }

  .rail-toggle :global(.mirrored) {
    transform: scaleX(-1);
  }

  .rail-filler:hover {
    background: var(--color-surface-hover);
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
