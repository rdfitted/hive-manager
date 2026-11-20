<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { Minus, Square, SidebarSimple, MagnifyingGlass, X } from 'phosphor-svelte';
  import { activeSession, serdeEnumVariantName, sessionDisplayName } from '$lib/stores/sessions';
  import { ui } from '$lib/stores/ui';
  import { layout } from '$lib/stores/layout';
  import { shortcutLabel } from '$lib/platform';
  import { IconButton, HiveMark } from '../ui';
  import PreviewControl from '../session/PreviewControl.svelte';
  import SessionHeader from '../session/SessionHeader.svelte';
  import ViewModeSwitch from './ViewModeSwitch.svelte';
  import { windowChrome } from './windowChrome';

  const chrome = windowChrome(typeof navigator === 'undefined' ? '' : navigator.userAgent);
  let modalOpen = $state(false);
  let windowError = $state('');
  let modalFocusTarget: HTMLButtonElement | null = null;
  const focusedAgent = $derived($activeSession?.agents.find((agent) => agent.id === ($ui.focusedAgentId || $ui.selectedAgentId)));
  const sessionLabel = $derived($activeSession ? sessionDisplayName($activeSession) : '');
  const sessionDetails = $derived($activeSession && [
    sessionLabel,
    serdeEnumVariantName($activeSession.session_type),
    `Session ID: ${$activeSession.id}`,
    $activeSession.project_path,
    $activeSession.created_at && `Created: ${new Date($activeSession.created_at).toLocaleString()}`,
  ].filter(Boolean).join('\n'));
  const sidebarLabel = $derived(`${$layout.leftCollapsed ? 'Expand' : 'Collapse'} sidebar`);
  const panelLabel = $derived(`${$layout.rightCollapsed ? 'Expand' : 'Collapse'} panel`);

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
  }

  function clearPendingAppControlFocus(event: FocusEvent | PointerEvent) {
    // A deferred dialog may not exist yet. Keep its focus intent until the user moves on.
    if (!modalFocusTarget || modalOpen || document.querySelector('[aria-modal="true"]')) return;
    if (event.type === 'focusout') {
      if (event.target === modalFocusTarget) modalFocusTarget = null;
    } else if (!(event.target instanceof Node) || !modalFocusTarget.contains(event.target)) {
      modalFocusTarget = null;
    }
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

<svelte:window onkeydowncapture={handleModalShortcut} onfocusoutcapture={clearPendingAppControlFocus} onpointerdowncapture={clearPendingAppControlFocus} />

<header class="title-bar" class:macos={chrome === 'native-traffic-lights'} data-tauri-drag-region data-window-chrome aria-label="Application title bar">
  {#if chrome === 'native-traffic-lights'}
    <div class="traffic-light-space" data-tauri-drag-region data-window-chrome></div>
  {/if}
  <div class="shell-actions" inert={modalOpen}>
    <IconButton id="title-bar-sidebar-toggle" icon={SidebarSimple} size="sm"
      label={`${sidebarLabel} (${shortcutLabel('B')})`} tooltip={sidebarLabel} shortcut={shortcutLabel('B')}
      onclick={() => layout.toggleLeft()} aria-expanded={!$layout.leftCollapsed} />
  </div>
  <div class="breadcrumb" data-tauri-drag-region data-window-chrome>
    <div class="brand-mark" data-tauri-drag-region data-window-chrome><HiveMark title="Hive" size={20} /></div>
    <span class="brand-wordmark" data-tauri-drag-region data-window-chrome>Hive Manager</span>
    {#if $activeSession}
      <span class="session-name" title={sessionDetails ?? undefined} data-tauri-drag-region data-window-chrome>{sessionLabel}</span>
      <SessionHeader session={$activeSession} />
    {/if}
    {#if focusedAgent}
      <span class="separator" data-tauri-drag-region data-window-chrome>/</span>
      <span class="agent-name" data-tauri-drag-region data-window-chrome>{focusedAgent.config.label || focusedAgent.id}</span>
    {/if}
  </div>
  <div class="shell-actions" inert={modalOpen}>
    {#if $activeSession}
      <ViewModeSwitch />
      <PreviewControl />
      <IconButton id="title-bar-panel-toggle" class="panel-toggle" icon={SidebarSimple} size="sm"
        label={`${panelLabel} (${shortcutLabel('J')})`} tooltip={panelLabel} shortcut={shortcutLabel('J')}
        onclick={() => layout.toggleRight()} aria-expanded={!$layout.rightCollapsed} />
    {/if}
    <IconButton icon={MagnifyingGlass} size="sm"
      label={`Open command palette (${shortcutLabel('Shift+P')})`} tooltip="Open command palette"
      shortcut={shortcutLabel('Shift+P')} onclick={openPalette} />
  </div>
  {#if chrome === 'custom-controls'}
    <div class="window-controls" data-tauri-drag-region data-window-chrome>
      <IconButton class="window-control" icon={Minus} label="Minimize window" tooltip="Minimize window" data-window-chrome onclick={() => windowAction('minimize')} />
      <IconButton class="window-control" icon={Square} label="Maximize or restore window" tooltip="Maximize or restore window" data-window-chrome onclick={() => windowAction('toggleMaximize')} />
      <IconButton class="window-control close-window" icon={X} label="Close window" tooltip="Close window" data-window-chrome onclick={() => windowAction('close')} />
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
    border-bottom: 1px solid var(--line);
    color: var(--ink-muted);
    user-select: none;
  }

  /* Reserve a caption layer above nested dialog stacks, even while body pointer events are locked. */
  [data-window-chrome] {
    position: relative;
    z-index: 10000;
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
  .shell-actions :global(svg), :global(.window-control svg) { pointer-events: none; }
  .brand-mark :global(svg) { pointer-events: none; }
  .brand-mark { display: flex; }
  .brand-wordmark { flex: none; white-space: nowrap; color: var(--text-primary); font-family: var(--font-display); font-size: var(--text-h3); font-weight: 600; }
  .breadcrumb :global([data-window-chrome]) { pointer-events: auto; }
  :global(.panel-toggle svg) { transform: scaleX(-1); }
  .breadcrumb { display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0; align-self: stretch; font-size: var(--text-small); }
  .session-name, .agent-name { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .session-name { color: var(--ink-muted); font-family: var(--font-display); font-size: var(--text-base); font-style: italic; }
  .agent-name { color: var(--ink-muted); }
  .separator { color: var(--ink-muted); }
  .traffic-light-space { flex: 0 0 68px; align-self: stretch; }
  .window-controls { display: flex; height: 100%; }
  .window-controls :global(button) { pointer-events: auto; }
  .window-controls :global(.window-control) { display: grid; place-items: center; width: 44px; height: 100%; padding: 0; border: 0; border-radius: 0; background: transparent; color: var(--text-primary); }
  .window-controls :global(.window-control:hover) { background: var(--toolbar-control); }
  .window-controls :global(.close-window:hover) { background: var(--danger-tint); color: var(--status-error); }
  .window-controls :global(.window-control) { transition: background-color var(--motion-ui), color var(--motion-ui); }
  .window-controls :global(.window-control:focus-visible) { outline: 2px solid var(--focus); outline-offset: -2px; }
  .window-error { position: fixed; top: var(--toolbar-height); right: 8px; z-index: 113; margin: 0; padding: 8px; background: var(--surface-overlay); color: var(--text-primary); }

  @media (max-width: 900px) { .agent-name, .separator { display: none; } }
  @media (forced-colors: active) { .title-bar { border-bottom-color: CanvasText; } .window-controls :global(.window-control:hover) { background: Highlight; color: HighlightText; } }
</style>
