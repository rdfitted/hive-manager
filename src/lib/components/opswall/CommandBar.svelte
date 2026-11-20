<script module lang="ts">
  import type { ComponentProps, Snippet } from 'svelte';
  import type ChatPane from '../chat/ChatPane.svelte';

  export type GridComposerRoute = NonNullable<ComponentProps<typeof ChatPane>['composerRoute']>;
</script>

<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { agentMentions, type MentionItem } from '$lib/composer/sources';
  import type { AgentInfo } from '$lib/stores/sessions';
  import { agentEvents } from '$lib/stores/agentEvents';
  import MentionMenu from '../composer/MentionMenu.svelte';
  import { toolbarFacts } from '../composer/toolbarFacts';

  let { sessionId, agents, defaultTargetId, enabled = true, targetId = $bindable(null), drafts = $bindable({}), ondelivery, children }: {
    sessionId: string;
    agents: AgentInfo[];
    defaultTargetId?: string;
    enabled?: boolean;
    children: Snippet<[GridComposerRoute]>;
    targetId?: string | null;
    drafts?: Record<string, string>;
    ondelivery?: (sessionId: string, agentId: string, state: string) => void;
  } = $props();

  const target = $derived(targetId === '@all' ? '@all' : agents.some(agent => agent.id === targetId) ? targetId! : defaultTargetId ?? agents[0]?.id ?? '');
  const selected = $derived(agents.find(agent => agent.id === target));
  const facts = $derived(toolbarFacts(selected, $agentEvents.sessionId === sessionId ? $agentEvents.feeds?.[target]?.capabilities.event_source : undefined));
  const items = $derived<MentionItem[]>([{ kind: 'agent', id: '@all', label: 'all', detail: 'Every agent once' }, ...agentMentions([...new Map(agents.map(agent => [agent.id, agent])).values()])]);
  const label = $derived(items.find(item => item.id === target)?.label ?? 'Choose target');
  const draftKey = $derived(`${sessionId}:${target}`);
  let sending = $state(false);
  let error = $state('');
  let menuOpen = $state(false);
  let activeIndex = $state(0);
  let x = $state(0);
  let y = $state(0);
  let picker: HTMLButtonElement;
  let bar: HTMLDivElement;
  $effect(() => { if (!enabled) menuOpen = false; });

  function choose(item: MentionItem) { targetId = item.id; menuOpen = false; }
  function openPicker() {
    const rect = picker.getBoundingClientRect();
    x = Math.max(8, Math.min(rect.left, window.innerWidth - 328));
    y = Math.max(8, rect.top - Math.min(240, items.length * 34 + 8) - 4);
    activeIndex = Math.max(0, items.findIndex(item => item.id === target));
    menuOpen = !menuOpen;
  }
  function cycleTarget(delta: number) {
    const index = items.findIndex(item => item.id === target);
    activeIndex = (index + delta + items.length) % items.length;
    targetId = items[activeIndex].id;
  }
  function keydown(event: KeyboardEvent): boolean {
    if (event.defaultPrevented || event.ctrlKey || event.metaKey || !(event.target instanceof HTMLElement)) return false;
    if (event.altKey) {
      if (agents.length && event.target.matches('[data-composer]') && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) {
        event.preventDefault(); cycleTarget(event.key === 'ArrowDown' ? 1 : -1); return true;
      }
      return false;
    }
    if (menuOpen) {
      if (event.key === 'Tab') {
        event.preventDefault(); activeIndex = (activeIndex + (event.shiftKey ? -1 : 1) + items.length) % items.length; targetId = items[activeIndex].id;
      } else if (event.key === 'Escape') { event.preventDefault(); menuOpen = false; }
      else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault(); activeIndex = (activeIndex + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
      } else if (event.key === 'Enter') { event.preventDefault(); choose(items[activeIndex]); }
      return event.defaultPrevented;
    }
    if (agents.length && event.target.matches('.target-picker') && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
      event.preventDefault(); cycleTarget(event.key === 'ArrowRight' ? 1 : -1); return true;
    }
    return false;
  }
  function captureKeydown(event: KeyboardEvent) {
    if (!enabled || !(event.target instanceof Node) || !bar?.contains(event.target)) return;
    const editorShortcut = event.altKey && event.target instanceof HTMLElement && event.target.matches('[data-composer]') && (event.key === 'ArrowUp' || event.key === 'ArrowDown');
    if ((menuOpen || editorShortcut) && keydown(event)) event.stopPropagation();
  }
  function prepareSubmit() {
    const sendingSession = sessionId;
    const ids = [...new Set(target === '@all' ? agents.map(agent => agent.id) : target ? [target] : [])];
    const key = draftKey;
    const original = drafts[key] ?? '';
    return {
      send: (text: string) => { void send(text, sendingSession, ids); },
      clear: () => { if ((drafts[key] ?? '') === original) drafts = { ...drafts, [key]: '' }; },
    };
  }
  async function send(text: string, sendingSession = sessionId, ids = [...new Set(target === '@all' ? agents.map(agent => agent.id) : target ? [target] : [])]) {
    if (sending || !text.trim()) return;
    if (!ids.length) return;
    sending = true; error = '';
    await Promise.allSettled(ids.map(async id => {
      ondelivery?.(sendingSession, id, 'Sending');
      try {
        await invoke('operator_inject', { request: { session_id: sendingSession, target_agent_id: id, message: text.trim() } });
        ondelivery?.(sendingSession, id, 'Sent');
      } catch (cause) {
        ondelivery?.(sendingSession, id, 'Send failed');
        if (sessionId === sendingSession) error = String(cause);
      }
    }));
    sending = false;
  }
</script>

<svelte:window onkeydowncapture={captureKeydown}
  onkeydown={(event) => { if (enabled && !menuOpen && event.target instanceof Node && bar?.contains(event.target)) keydown(event); }}
  onpointerdown={(event) => { if (menuOpen && event.target instanceof Element && !event.target.closest('.target-picker, .mention-menu')) menuOpen = false; }} />

{#snippet targetControl()}
  <button bind:this={picker} type="button" class="target-picker lattice-btn lattice-btn--ghost"
    aria-label={`Command target: @${label}`} aria-haspopup="listbox" aria-expanded={menuOpen} disabled={!agents.length}
    title="Choose message target (Alt+↑/↓ changes target)" onmousedown={(event) => event.preventDefault()} onclick={openPicker}>@{label}</button>
  {#if menuOpen}<MentionMenu {items} {activeIndex} {x} {y} onselect={choose} />{/if}
{/snippet}

<div bind:this={bar} class="command-bar" class:routing-active={enabled}>
  {@render children({
    agentId: target, placeholder: target === '@all' ? 'Message all agents' : facts.placeholder,
    disabled: sending || !agents.length, sending, prepareSubmit, onsubmit: send, targetControl, error,
  })}
</div>

<style>
  .command-bar { height: 100%; min-width: 0; min-height: 0; }
  .routing-active :global(.composer-toolbar) { display: flex; gap: var(--space-1); flex-wrap: wrap; align-items: center; }
  .routing-active :global(.composer-toolbar > .toolbar) { flex: 1 1 auto; }
  .target-picker { max-width: 55%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
