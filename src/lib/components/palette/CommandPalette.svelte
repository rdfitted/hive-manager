<script lang="ts">
  import { tick } from 'svelte';
  import { sessions, activeSession } from '$lib/stores/sessions';
  import { ui } from '$lib/stores/ui';
  import { layout, type RightPanelTab } from '$lib/stores/layout';
  import { shell } from '$lib/stores/shell';
  import { flushEnter } from '../attention/flush';

  interface Command { id: string; label: string; run: () => void | Promise<void>; keepOpen?: boolean }
  let open = $state(false);
  let query = $state('');
  let selected = $state(0);
  let input = $state<HTMLInputElement>();
  let dialog = $state<HTMLDivElement>();
  let previousFocus: HTMLElement | null = null;
  let pending = $state(false);
  let receipt = $state('');
  let error = $state('');
  let receiptTarget = $state('');
  let opening = 0;
  const focusedAgent = $derived($activeSession?.agents.find((agent) => agent.id === $ui.focusedAgentId));
  const commands = $derived.by((): Command[] => {
    const tabs: RightPanelTab[] = ['overview', 'activity', 'changes'];
    return [
      { id: 'new-session', label: 'New session', run: () => shell.requestStartAction('hive') },
      ...($activeSession ? [{ id: 'add-worker', label: 'Add worker', run: () => shell.openAddWorker() }] : []),
      { id: 'view-mode', label: 'Toggle Focus / Grid', run: () => layout.toggleSessionViewMode() },
      { id: 'toggle-rail', label: 'Toggle right panel', run: () => layout.toggleRight() },
      ...tabs.map((tab) => ({ id: `tab-${tab}`, label: `Open ${tab[0].toUpperCase()}${tab.slice(1)}`, run: () => layout.setRightTab(tab) })),
      { id: 'shortcuts', label: 'Open keyboard shortcuts', run: () => { window.dispatchEvent(new KeyboardEvent('keydown', { key: '/', ctrlKey: true, bubbles: true })); } },
      ...$sessions.sessions.map((session) => ({ id: `session-${session.id}`, label: `Switch session: ${session.name || session.project_path || session.id}`, run: () => sessions.setActiveSession(session.id) })),
      ...($activeSession?.agents ?? []).map((agent) => ({ id: `agent-${agent.id}`, label: `Focus agent: ${agent.config.label || agent.config.name || agent.id}`, run: () => ui.setFocusedAgent(agent.id) })),
      ...(focusedAgent && $activeSession ? [{ id: 'flush', label: `Flush Enter: ${focusedAgent.config.label || focusedAgent.id}`, keepOpen: true, run: flushFocused }] : []),
    ];
  });
  const matches = $derived(commands.filter((command) => command.label.toLowerCase().includes(query.trim().toLowerCase())));
  const activeIndex = $derived(Math.min(selected, Math.max(0, matches.length - 1)));

  async function show() {
    if (open) return;
    previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    opening += 1;
    query = ''; selected = 0; receipt = ''; error = ''; open = true;
    await tick();
    input?.focus();
  }

  function close() {
    opening += 1;
    open = false;
    if (previousFocus?.isConnected) previousFocus.focus();
  }

  function handleWindowKey(event: KeyboardEvent) {
    if (event.repeat || event.defaultPrevented || event.altKey) return;
    const modifier = event.ctrlKey || event.metaKey;
    const key = event.key.toLowerCase();
    const insideTerminal = event.target instanceof Element && !!event.target.closest('.xterm');
    if (modifier && ((key === 'k' && !event.shiftKey && !insideTerminal) || (key === 'p' && event.shiftKey))) {
      event.preventDefault();
      if (open) close(); else void show();
    }
  }

  function handleViewKey(event: KeyboardEvent) {
    if (!event.defaultPrevented && !event.repeat && event.altKey && event.shiftKey && !event.ctrlKey && !event.metaKey && event.key === 'F6') {
      event.preventDefault();
      layout.toggleSessionViewMode();
    }
  }

  async function flushFocused() {
    const session = $activeSession;
    const agent = focusedAgent;
    if (!session || !agent || pending) return;
    pending = true; receipt = ''; error = '';
    const sentOpening = opening;
    receiptTarget = agent.config.label || agent.id;
    try {
      const body = await flushEnter(session.id, agent.id);
      if (opening === sentOpening) receipt = body;
    }
    catch (cause) { if (opening === sentOpening) error = String(cause); }
    finally { pending = false; }
  }

  function execute(command: Command | undefined) {
    if (!command) return;
    if (!command.keepOpen) close();
    void command.run();
  }

  function handleDialogKey(event: KeyboardEvent) {
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); }
    else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      event.stopPropagation();
      selected = matches.length ? (activeIndex + (event.key === 'ArrowDown' ? 1 : matches.length - 1)) % matches.length : 0;
      input?.focus();
      dialog?.querySelector(`[data-command-index="${selected}"]`)?.scrollIntoView?.({ block: 'nearest' });
    } else if (event.key === 'Enter' && event.target === input) {
      event.preventDefault(); execute(matches[activeIndex]);
    } else if (event.key === 'Tab') {
      const controls = Array.from(dialog?.querySelectorAll<HTMLElement>('input, button:not(:disabled)') ?? []);
      const first = controls[0]; const last = controls.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }
  }
</script>

<!-- Bubble phase preserves xterm's ownership of Ctrl+K. -->
<svelte:window onkeydown={(event) => { handleWindowKey(event); handleViewKey(event); }} />

{#if open}
  <div class="backdrop lattice-modal-backdrop" role="presentation" onclick={(event) => { if (event.target === event.currentTarget) close(); }}>
    <div bind:this={dialog} class="palette lattice-modal" role="dialog" aria-modal="true" aria-label="Command palette" tabindex="-1" onkeydown={handleDialogKey}>
      <div class="search-row lattice-forced-colors-boundary">
        <label for="command-search">Commands</label>
        <button type="button" class="lattice-btn lattice-btn--ghost" onclick={close} aria-label="Close command palette">Esc</button>
      </div>
      <input bind:this={input} id="command-search" class="lattice-input" aria-label="Find command" aria-describedby="command-selection" placeholder="Find a command…" bind:value={query} oninput={() => { selected = 0; }} />
      <span id="command-selection" class="selection-announcement" aria-live="polite">{matches[activeIndex]?.label ?? 'No matching commands'}</span>
      <ul class="commands lattice-scroll-content" aria-label="Commands">
        {#each matches as command, index (command.id)}
          <li><button type="button" data-command-index={index} class="command lattice-forced-colors-boundary" class:active={index === activeIndex} aria-current={index === activeIndex ? 'true' : undefined} aria-disabled={pending && command.id === 'flush' ? 'true' : undefined} onclick={() => execute(command)}>{command.label}</button></li>
        {:else}<li class="empty">No commands match “{query}”.</li>{/each}
      </ul>
      {#if pending}<p role="status">Flushing Enter to {receiptTarget}…</p>{/if}
      {#if receipt}<div class="result"><p>Inject receipt · {receiptTarget}</p><pre role="status" aria-label="Inject receipt">{receipt}</pre></div>{/if}
      {#if error}<p class="result" role="alert">{error}</p>{/if}
      <p class="hint">↑ ↓ to choose · Enter to run · Esc to close</p>
    </div>
  </div>
{/if}

<style>
  .backdrop { position: fixed; inset: 0; z-index: 110; display: flex; justify-content: center; align-items: flex-start; padding: min(14vh, 6rem) var(--space-4) var(--space-4); background: color-mix(in srgb, var(--bg-void) 60%, transparent); }
  .palette { width: 34rem; max-width: 100%; max-height: 75vh; display: flex; flex-direction: column; padding: var(--space-3); gap: var(--space-2); }
  .search-row { display: flex; align-items: center; justify-content: space-between; color: var(--text-secondary); font-size: var(--text-small); }
  .commands { margin: 0; padding: 0; list-style: none; overflow-y: auto; min-height: 0; }
  .command { display: block; width: 100%; padding: var(--space-3); border: 0; border-radius: var(--radius-sm); background: transparent; color: var(--text-primary); text-align: left; font: inherit; font-size: var(--text-small); cursor: pointer; }
  .command.active, .command:hover { background: var(--bg-elevated); }
  .command:focus-visible { outline: 2px solid var(--accent-cyan); outline-offset: -2px; }
  .empty, .hint { color: var(--text-secondary); padding: var(--space-2); font-size: var(--text-small); }
  .hint { margin: 0; }
  .selection-announcement { position: absolute; width: 1px; height: 1px; padding: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .result { margin: 0; font-size: var(--text-small); }
  .result p { margin: 0; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 8rem; overflow: auto; }
</style>
