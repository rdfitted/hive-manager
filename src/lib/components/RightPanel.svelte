<script lang="ts">
  import { X } from 'phosphor-svelte';
  import { layout, COLLAPSED_PANEL_WIDTH, type RightPanelTab } from '$lib/stores/layout';
  import { shortcutLabel } from '$lib/platform';
  import OverviewTab from './rail/OverviewTab.svelte';
  import ActivityTab from './rail/ActivityTab.svelte';
  import ChangesTab from './rail/ChangesTab.svelte';
  import ResizeHandle from './ResizeHandle.svelte';

  const TABS: Array<{ id: RightPanelTab; label: string }> = [
    { id: 'overview', label: 'Overview' },
    { id: 'activity', label: 'Activity' },
    { id: 'changes', label: 'Changes' },
  ];
  const collapsed = $derived($layout.rightCollapsed);
  const activeTab = $derived($layout.rightTab);
  const width = $derived(collapsed ? COLLAPSED_PANEL_WIDTH : $layout.rightWidth);

  function handleResize(clientX: number) {
    layout.setRightWidth(window.innerWidth - clientX);
  }
</script>

<aside class="right-panel" style:width={`${width}px`} style:min-width={`${width}px`} style:flex-basis={`${width}px`}>
  <div class="panel-drawer lattice-forced-colors-boundary" class:collapsed style:width={`${$layout.rightWidth}px`} inert={collapsed} aria-hidden={collapsed}>
    <div class="panel-header">
      <div class="panel-tabs" role="group" aria-label="Panel views">
        {#each TABS as tab (tab.id)}
          <button
            type="button"
            class={`panel-tab lattice-btn lattice-btn--compact ${activeTab === tab.id ? 'lattice-btn--secondary' : 'lattice-btn--ghost'}`}
            onclick={() => layout.activateRightTab(tab.id)}
            aria-label={tab.label}
            aria-pressed={activeTab === tab.id && !collapsed}
          >{tab.label}</button>
        {/each}
      </div>
      <button type="button" class="lattice-btn lattice-btn--ghost lattice-btn--icon" onclick={() => layout.toggleRight()} aria-label={`Collapse panel (${shortcutLabel('J')})`}>
        <X size={16} aria-hidden="true" />
      </button>
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
    <ResizeHandle label="Resize panel" onResize={handleResize} />
  </div>
</aside>

<style>
  .right-panel {
    position: relative;
    display: flex;
    height: 100%;
    flex-shrink: 0;
    overflow: hidden;
    background: var(--chrome);
    z-index: 20;
  }
  .panel-drawer {
    position: relative;
    display: flex;
    flex-shrink: 0;
    min-width: 0;
    flex-direction: column;
    background: var(--surface);
    border-left: 1px solid var(--border);
  }
  .panel-drawer.collapsed { visibility: hidden; pointer-events: none; }
  .panel-drawer :global(.resize-handle) { left: 0; }
  .panel-header { display: flex; align-items: center; gap: 4px; min-height: var(--toolbar-height); padding: 4px 8px; border-bottom: 1px solid var(--border); }
  .panel-tabs { display: flex; flex: 1; min-width: 0; gap: 2px; }
  .panel-tab { min-width: 0; padding-inline: 6px; font-size: var(--text-small); }
  .panel-title { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .tab-content { flex: 1; min-height: 0; overflow: hidden; display: flex; flex-direction: column; }
  @media (forced-colors: active) { .panel-drawer, .panel-header { border-color: CanvasText; } }
</style>
