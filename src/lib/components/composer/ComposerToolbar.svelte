<script lang="ts">
  import { tick } from 'svelte';
  import { Portal } from 'bits-ui';
  import { ArrowUp, Plus } from 'phosphor-svelte';
  import type { ComposerControls } from './Composer.svelte';

  let { controls, accessLabel, model, effort, sending, targetLabel }: {
    controls: ComposerControls;
    accessLabel?: string;
    model?: string;
    effort?: string;
    sending: boolean;
    targetLabel?: string;
  } = $props();

  const targetId = $props.id();
  let menuOpen = $state(false);
  let menuX = $state(0);
  let menuY = $state(0);
  let addButton: HTMLButtonElement;
  let menu: HTMLDivElement | undefined = $state();
  $effect(() => { if (sending) menuOpen = false; });

  function toggleMenu() {
    if (sending) return;
    const rect = addButton.getBoundingClientRect();
    menuX = rect.left;
    menuY = rect.top;
    menuOpen = !menuOpen;
  }

  function choose(mode: 'mention' | 'slash') {
    menuOpen = false;
    controls.openMenu(mode);
  }

  function send() {
    if (controls.canSend && !sending) controls.submit();
  }

  function outsidePointer(event: PointerEvent) {
    if (event.target instanceof Node && !menu?.contains(event.target) && !addButton?.contains(event.target)) {
      menuOpen = false;
    }
  }

  async function addKeydown(event: KeyboardEvent) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!menuOpen) toggleMenu();
      await tick();
      const items = menu?.querySelectorAll<HTMLButtonElement>('button');
      items?.[event.key === 'ArrowDown' ? 0 : items.length - 1]?.focus();
    } else if (event.key === 'Escape') menuOpen = false;
  }

  function menuKeydown(event: KeyboardEvent) {
    const items = Array.from(menu?.querySelectorAll<HTMLButtonElement>('button') ?? []);
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      menuOpen = false;
      addButton.focus();
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const index = items.indexOf(document.activeElement as HTMLButtonElement);
      items[(index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length]?.focus();
    } else if (event.key === 'Tab') menuOpen = false;
  }
</script>

<svelte:window onpointerdown={outsidePointer} onkeydown={(event) => { if (event.key === 'Escape') menuOpen = false; }} />

<div class="toolbar" role="group" aria-label="Message controls" onpointerdown={(event) => event.preventDefault()}>
  {#if targetLabel}
    <span class="message-target" id={targetId} title={`Send to ${targetLabel}`}><span class="visually-hidden">Send to </span><span aria-hidden="true">@</span>{targetLabel}</span>
  {/if}
  <button bind:this={addButton} type="button" class="toolbar-button add-button" aria-label="Add mention or command" title="Add mention or command" aria-haspopup="menu" aria-expanded={menuOpen} disabled={sending} onmousedown={(event) => event.preventDefault()} onclick={toggleMenu} onkeydown={addKeydown}>
    <Plus size={14} aria-hidden="true" />
  </button>
  {#if accessLabel}<span class="access-label">{accessLabel}</span>{/if}
  <div class="model-facts">
    {#if model}<span class="model">{model}</span>{/if}
    {#if effort}<span class="effort">{effort}</span>{/if}
  </div>
  <button type="button" class="toolbar-button send-button" aria-label={sending ? 'Sending message' : 'Send message'} title={sending ? 'Sending message' : 'Send message'} aria-describedby={targetLabel ? targetId : undefined} aria-busy={sending} disabled={!controls.canSend || sending} onmousedown={(event) => event.preventDefault()} onclick={send}>
    <ArrowUp size={14} weight="bold" aria-hidden="true" />
  </button>
</div>

{#if menuOpen}
  <Portal>
    <div bind:this={menu} class="add-menu lattice-panel" role="menu" tabindex="-1" aria-label="Add to message" style:left="{menuX}px" style:top="{menuY}px" onpointerdown={(event) => event.stopPropagation()} onmousedown={(event) => event.preventDefault()} onkeydown={menuKeydown}>
      <button type="button" role="menuitem" class="lattice-btn lattice-btn--menu-item" onclick={() => choose('mention')}>Mention</button>
      <button type="button" role="menuitem" class="lattice-btn lattice-btn--menu-item" onclick={() => choose('slash')}>Command</button>
    </div>
  </Portal>
{/if}

<style>
  .toolbar { display: flex; align-items: center; gap: 6px; min-width: 0; }
  /* The target is the one accent chip in the composer. */
  .message-target { display: inline-flex; align-items: center; min-width: 0; height: 24px; padding: 0 10px; overflow: hidden; border-radius: 12px; background: var(--accent-bg); color: var(--accent); font-size: 11.5px; text-overflow: ellipsis; white-space: nowrap; }
  .visually-hidden { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .toolbar-button { display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0; width: 26px; height: 26px; padding: 0; border: 1px solid transparent; border-radius: var(--radius-full); background: transparent; color: var(--ink-muted); cursor: pointer; transition: background-color var(--motion-ui), color var(--motion-ui); }
  .toolbar-button:hover:enabled { background: var(--wash-strong); color: var(--ink); }
  .toolbar-button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .toolbar-button:disabled { opacity: 0.45; cursor: default; }
  .access-label { color: var(--status-warning); font-size: 11px; white-space: nowrap; }
  .model-facts { display: flex; flex-wrap: wrap; justify-content: flex-end; align-items: baseline; gap: 0 var(--space-2); margin-left: auto; min-width: 0; font: 11px/1.3 var(--font-mono); }
  .model { overflow-wrap: anywhere; color: var(--ink-muted); }
  .effort { color: var(--ink-dim); }
  .send-button, .send-button:hover:enabled { background: var(--accent); color: var(--accent-ink); }
  .add-menu { position: fixed; z-index: 200; pointer-events: auto; transform: translateY(-100%); display: flex; flex-direction: column; min-width: 160px; padding: var(--space-1); box-shadow: var(--elev-3); }
  @media (forced-colors: active) {
    .toolbar-button, .add-menu { border: 1px solid ButtonText; }
    .toolbar-button:disabled { color: GrayText; opacity: 1; }
    .toolbar-button:focus-visible { outline-color: Highlight; }
    .send-button { background: ButtonFace; color: ButtonText; }
    .message-target { border: 1px solid CanvasText; }
  }
</style>
