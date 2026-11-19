<script lang="ts">
  import { tick } from 'svelte';
  import { Archive, ChatCircle, ClockCounterClockwise, Files, Gauge, GitDiff, Graph, ListBullets, ListChecks, Snowflake, Sun, X } from 'phosphor-svelte';
  import { accent } from '$lib/stores/accent';
  import { layout, COLLAPSED_PANEL_WIDTH, type RightPanelTab } from '$lib/stores/layout';
  import { shortcutLabel } from '$lib/platform';
  import OverviewTab from './rail/OverviewTab.svelte';
  import ActivityTab from './rail/ActivityTab.svelte';
  import ChangesTab from './rail/ChangesTab.svelte';
  import ResizeHandle from './ResizeHandle.svelte';
  import { IconButton } from './ui';

  const TABS = [
    { id: 'overview', label: 'Overview', icon: Gauge },
    { id: 'activity', label: 'Activity', icon: ChatCircle },
    { id: 'changes', label: 'Changes', icon: GitDiff },
    { id: 'plan', label: 'Plan', icon: ListChecks },
    { id: 'graph', label: 'Graph', icon: Graph },
    { id: 'logs', label: 'Logs', icon: ListBullets },
    { id: 'timeline', label: 'Timeline', icon: ClockCounterClockwise },
    { id: 'files', label: 'Files', icon: Files },
    { id: 'artifacts', label: 'Artifacts', icon: Archive },
  ] as const;
  const collapsed = $derived($layout.rightCollapsed);
  const activeTab = $derived($layout.rightTab);
  const width = $derived(collapsed ? COLLAPSED_PANEL_WIDTH : $layout.rightWidth);
  const accentName = $derived($accent === 'honey' ? 'Honey' : 'Sky');

  function handleResize(clientX: number) {
    layout.setRightWidth(window.innerWidth - clientX);
  }

  async function collapsePanel() {
    layout.toggleRight();
    await tick();
    document.getElementById('title-bar-panel-toggle')?.focus({ preventScroll: true });
  }

  async function activateTab(tab: RightPanelTab) {
    const willCollapse = !collapsed && activeTab === tab;
    layout.activateRightTab(tab);
    if (willCollapse) {
      await tick();
      document.getElementById('title-bar-panel-toggle')?.focus({ preventScroll: true });
    }
  }

  function inspectorEscape(node: HTMLDivElement) {
    function onKeydown(event: KeyboardEvent) {
      if (event.key !== 'Escape' || event.defaultPrevented || collapsed ||
          document.querySelector('[aria-modal="true"]')) return;
      event.preventDefault();
      void collapsePanel();
    }
    // Let nested controls handle Escape first, before document-level tooltips.
    node.addEventListener('keydown', onKeydown);
    return { destroy: () => node.removeEventListener('keydown', onKeydown) };
  }
</script>

<aside class="right-panel" style:width={`${width}px`} style:min-width={`${width}px`} style:flex-basis={`${width}px`}>
  <div class="panel-drawer lattice-forced-colors-boundary" class:collapsed use:inspectorEscape inert={collapsed} aria-hidden={collapsed}>
    <div class="panel-header">
      <div class="panel-tabs" role="group" aria-label="Panel views">
        {#each TABS as tab (tab.id)}
          <IconButton class="panel-tab" size="sm" icon={tab.icon} label={tab.label} tooltip={tab.label}
            pressed={activeTab === tab.id && !collapsed} onclick={() => activateTab(tab.id)} />
        {/each}
      </div>
      <IconButton size="sm" icon={X} label={`Collapse panel (${shortcutLabel('J')})`} tooltip="Collapse panel"
        shortcut="Esc" onclick={collapsePanel} />
      <h2 class="panel-title">{TABS.find((tab) => tab.id === activeTab)?.label}</h2>
    </div>
    <div class="tab-content">
      {#if activeTab === 'overview' || activeTab === 'plan' || activeTab === 'graph'}
        <OverviewTab tab={activeTab} />
      {:else if activeTab === 'activity' || activeTab === 'logs' || activeTab === 'timeline'}
        <ActivityTab tab={activeTab} />
      {:else}
        <ChangesTab tab={activeTab} />
      {/if}
    </div>
    <div class="panel-footer">
      <IconButton size="sm" icon={$accent === 'honey' ? Sun : Snowflake} label={`Switch accent (current: ${accentName})`}
        tooltip={`Switch accent (current: ${accentName})`} onclick={accent.toggle} />
    </div>
  </div>
  {#if !collapsed}<ResizeHandle label="Resize panel" onResize={handleResize} />{/if}
</aside>

<style>
  .right-panel {
    position: relative;
    display: flex;
    height: 100%;
    flex-shrink: 0;
    overflow: hidden;
    z-index: 20;
  }
  .panel-drawer {
    position: relative;
    display: flex;
    flex: 1;
    min-width: 0;
    flex-direction: column;
    margin: var(--space-2);
    background: var(--surface);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--depth-1);
  }
  @supports ((backdrop-filter: blur(1px)) or (-webkit-backdrop-filter: blur(1px))) {
    .panel-drawer {
      background: var(--glass-strong);
      -webkit-backdrop-filter: blur(14px);
      backdrop-filter: blur(14px);
    }
  }
  .panel-drawer.collapsed { visibility: hidden; pointer-events: none; }
  .right-panel :global(.resize-handle) { left: 0; }
  .panel-header { display: flex; align-items: flex-start; gap: var(--space-2); padding: var(--space-2); border-bottom: 1px solid var(--line); }
  .panel-tabs { display: flex; flex: 1; flex-wrap: wrap; min-width: 0; gap: var(--space-1); }
  .panel-title { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .tab-content { flex: 1; min-height: 0; overflow: hidden; display: flex; flex-direction: column; }
  .panel-footer { padding: var(--space-2); border-top: 1px solid var(--line); }
  @media (forced-colors: active) {
    .panel-drawer, .panel-header, .panel-footer { border-color: CanvasText; }
    .panel-drawer { background: Canvas; box-shadow: none; -webkit-backdrop-filter: none; backdrop-filter: none; }
  }
</style>
