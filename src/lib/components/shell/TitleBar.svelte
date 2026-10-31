<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { Minus, Square, SidebarSimple, MagnifyingGlass, X } from 'phosphor-svelte';
  import { activeSession } from '$lib/stores/sessions';
  import { ui } from '$lib/stores/ui';
  import { layout } from '$lib/stores/layout';
  import { shortcutLabel } from '$lib/platform';
  import ViewModeSwitch from './ViewModeSwitch.svelte';
  import { windowChrome } from './windowChrome';

  const chrome = windowChrome(typeof navigator === 'undefined' ? '' : navigator.userAgent);
  let modalOpen = $state(false);
  let windowError = $state('');
  let modalFocusTarget: HTMLButtonElement | null = null;
  const focusedAgent = $derived($activeSession?.agents.find((agent) => agent.id === ($ui.focusedAgentId || $ui.selectedAgentId)));
  const sessionLabel = $derived($activeSession?.name || $activeSession?.project_path.split(/[\\/]/).filter(Boolean).at(-1) || 'Hive Manager');
  const panelLabel = $derived(`${$layout.rightCollapsed ? 'Expand' : 'Collapse'} panel (${shortcutLabel('J')})`);

  onMount(() => {
    const syncModal = () => {
      const wasOpen = modalOpen;
      modalOpen = document.querySelector('[aria-modal="true"]') !== null;
      if (wasOpen && !modalOpen && modalFocusTarget) {
        const target = modalFocusTarget;
        modalFocusTarget = null;
        // Bits restores focus during unmount, before this observer can clear inert.
        void tick().then(() => {
          if (target.isConnected && !target.closest('[inert]') && document.activeElement === document.body && !document.querySelector('[aria-modal="true"]')) {
            target.focus({ preventScroll: true });
          }
        });
      }
    };
    syncModal();
    const observer = new MutationObserver(syncModal);
    observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['aria-modal'] });
    return () => observer.disconnect();
  });

  async function windowAction(action: 'minimize' | 'toggleMaximize' | 'close') {
    try {
      windowError = '';
      await getCurrentWindow()[action]();
    } catch {
      windowError = 'Window action failed. Try again.';
    }
  }

  function rememberAppControlFocus() {
    const active = document.activeElement;
    modalFocusTarget = active instanceof HTMLButtonElement && active.closest('.title-bar .shell-actions') ? active : null;
    void tick().then(() => {
      if (!document.querySelector('[aria-modal="true"]')) modalFocusTarget = null;
    });
  }

  function handleModalShortcut(event: KeyboardEvent) {
    if (modalOpen || event.repeat || event.defaultPrevented || !(event.ctrlKey || event.metaKey)) return;
    const key = event.key.toLowerCase();
    if (key === '/' || (key === 'k' && !event.shiftKey) || (key === 'p' && event.shiftKey)) rememberAppControlFocus();
  }

  function openPalette() {
    rememberAppControlFocus();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'P', ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true }));
  }
</script>

<svelte:window onkeydowncapture={handleModalShortcut} />

<header class="title-bar" class:macos={chrome === 'native-traffic-lights'} data-tauri-drag-region data-window-chrome aria-label="Application title bar">
  {#if chrome === 'native-traffic-lights'}
    <div class="traffic-light-space" data-tauri-drag-region data-window-chrome></div>
  {/if}
  <div class="shell-actions" inert={modalOpen}>
    <button type="button" class="lattice-btn lattice-btn--ghost lattice-btn--icon" onclick={() => layout.toggleLeft()} aria-label={`${$layout.leftCollapsed ? 'Expand' : 'Collapse'} sidebar (${shortcutLabel('B')})`} aria-expanded={!$layout.leftCollapsed}>
      <SidebarSimple size={18} aria-hidden="true" />
    </button>
  </div>
  <div class="breadcrumb" data-tauri-drag-region data-window-chrome>
    <span class="session-name" data-tauri-drag-region data-window-chrome>{sessionLabel}</span>
    {#if focusedAgent}
      <span class="separator" data-tauri-drag-region data-window-chrome>/</span>
      <span class="agent-name" data-tauri-drag-region data-window-chrome>{focusedAgent.config.label || focusedAgent.id}</span>
    {/if}
  </div>
  <div class="shell-actions" inert={modalOpen}>
    {#if $activeSession}
      <ViewModeSwitch />
      <button type="button" class="panel-toggle lattice-btn lattice-btn--ghost lattice-btn--icon" onclick={() => layout.toggleRight()} aria-label={panelLabel} aria-expanded={!$layout.rightCollapsed}>
        <SidebarSimple size={18} aria-hidden="true" />
      </button>
    {/if}
    <button type="button" class="lattice-btn lattice-btn--ghost lattice-btn--icon" onclick={openPalette} aria-label={`Open command palette (${shortcutLabel('Shift+P')})`}>
      <MagnifyingGlass size={18} aria-hidden="true" />
    </button>
  </div>
  {#if chrome === 'custom-controls'}
    <div class="window-controls" data-tauri-drag-region data-window-chrome>
      <button type="button" class="window-control" data-window-chrome aria-label="Minimize window" onclick={() => windowAction('minimize')}><Minus size={16} aria-hidden="true" /></button>
      <button type="button" class="window-control" data-window-chrome aria-label="Maximize or restore window" onclick={() => windowAction('toggleMaximize')}><Square size={14} aria-hidden="true" /></button>
      <button type="button" class="window-control close-window" data-window-chrome aria-label="Close window" onclick={() => windowAction('close')}><X size={16} aria-hidden="true" /></button>
    </div>
  {/if}
</header>
{#if windowError}<p class="window-error" role="status">{windowError}</p>{/if}

<style>
  .title-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 0 0 var(--toolbar-height);
    height: var(--toolbar-height);
    min-width: 0;
    padding-left: var(--sidebar-inset);
    background: var(--chrome);
    border-bottom: 1px solid var(--border);
    color: var(--text-secondary);
    user-select: none;
  }

  /* Bits disables body pointer events. Native chrome must remain usable above dialogs. */
  [data-window-chrome] {
    position: relative;
    z-index: 112;
    pointer-events: auto;
  }

  .shell-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    pointer-events: none;
  }

  .shell-actions:not([inert]) :global(button) { pointer-events: auto; }
  .shell-actions[inert] { opacity: 0.5; }
  .shell-actions :global(svg), .window-control :global(svg) { pointer-events: none; }
  .panel-toggle :global(svg) { transform: scaleX(-1); }
  .breadcrumb { display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0; align-self: stretch; font-size: var(--text-small); }
  .session-name, .agent-name { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .session-name { color: var(--text-primary); }
  .agent-name { color: var(--text-muted); }
  .separator { color: var(--text-muted); }
  .traffic-light-space { flex: 0 0 68px; align-self: stretch; }
  .window-controls { display: flex; height: 100%; }
  .window-control { display: grid; place-items: center; width: 44px; padding: 0; border: 0; background: transparent; color: var(--text-primary); }
  .window-control:hover { background: var(--toolbar-control); }
  .close-window:hover { background: var(--status-error); color: var(--bg-void); }
  .window-error { position: fixed; top: var(--toolbar-height); right: 8px; z-index: 113; margin: 0; padding: 8px; background: var(--surface-overlay); color: var(--text-primary); }

  @media (max-width: 900px) { .agent-name, .separator { display: none; } }
  @media (forced-colors: active) { .title-bar { border-bottom-color: CanvasText; } .window-control:hover { background: Highlight; color: HighlightText; } }
</style>
