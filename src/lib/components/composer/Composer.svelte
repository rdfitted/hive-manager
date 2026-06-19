<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { get } from 'svelte/store';
  import MentionMenu from './MentionMenu.svelte';
  import SlashMenu from './SlashMenu.svelte';
  import {
    agentMentions,
    sessionMentions,
    filterMentions,
    flattenMention,
    type MentionItem,
  } from '$lib/composer/sources';
  import { filterCommands, type SlashCommand } from '$lib/composer/commands';
  import { composerDraft } from '$lib/stores/composerDraft';
  import { pendingContext } from '$lib/stores/pendingContext';
  import { activeAgents, sessions as sessionsStore } from '$lib/stores/sessions';

  interface Props {
    sessionId: string | null;
    agentId?: string | null;
    placeholder?: string;
    /** Bind the flattened plain-text value (two-way for dialogs that read it on their own submit). */
    value?: string;
    /** Whether to persist drafts per-session (default true). Dialogs may opt out. */
    persistDraft?: boolean;
    /** Called with the flattened plain-text prompt (one-shot context already prepended). */
    onsubmit?: (text: string) => void;
    disabled?: boolean;
  }

  let {
    sessionId,
    agentId = null,
    placeholder = 'Message…',
    value = $bindable(''),
    persistDraft = true,
    onsubmit,
    disabled = false,
  }: Props = $props();

  let editor: HTMLDivElement;
  let menuMode: 'none' | 'mention' | 'slash' = $state('none');
  let menuQuery = $state('');
  let menuX = $state(0);
  let menuY = $state(0);
  let activeIndex = $state(0);
  /** Start offset of the active @/ trigger within the current text node. */
  let triggerStart = -1;
  let triggerNode: Node | null = null;

  let mentionItems: MentionItem[] = $state([]);
  let commandItems: SlashCommand[] = $state([]);

  // --- Draft load on session change ---
  let lastBoundSession: string | null = null;
  $effect(() => {
    if (!persistDraft) return;
    if (sessionId && sessionId !== lastBoundSession) {
      lastBoundSession = sessionId;
      const initial = composerDraft.load(sessionId);
      value = initial;
      // Reflect into the editor DOM if mounted.
      if (editor && editor.textContent !== initial) {
        editor.textContent = initial;
      }
    }
  });

  onMount(() => {
    if (persistDraft && sessionId) {
      const initial = composerDraft.load(sessionId);
      value = initial;
      lastBoundSession = sessionId;
    }
    if (editor && value) editor.textContent = value;
  });

  function currentText(): string {
    return editor?.textContent ?? '';
  }

  function syncValue() {
    value = currentText();
    if (persistDraft && sessionId) {
      composerDraft.update(value);
    }
  }

  /** Compute caret anchor in viewport pixels for the menu position. */
  function caretRect(): { x: number; y: number } {
    const sel = window.getSelection();
    if (sel && sel.rangeCount > 0) {
      const range = sel.getRangeAt(0).cloneRange();
      const rects = range.getClientRects();
      const rect = rects.length > 0 ? rects[0] : editor.getBoundingClientRect();
      return { x: rect.left, y: rect.bottom + 4 };
    }
    const r = editor.getBoundingClientRect();
    return { x: r.left, y: r.bottom + 4 };
  }

  /** Detect an active @ or / trigger immediately before the caret in the current text node. */
  function detectTrigger() {
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0) {
      closeMenu();
      return;
    }
    const range = sel.getRangeAt(0);
    const node = range.startContainer;
    if (node.nodeType !== Node.TEXT_NODE) {
      closeMenu();
      return;
    }
    const text = node.textContent ?? '';
    const offset = range.startOffset;
    // Walk back from the caret to find a trigger char not preceded by a word char.
    let i = offset - 1;
    while (i >= 0) {
      const ch = text[i];
      if (ch === '@' || ch === '/') {
        const prev = i > 0 ? text[i - 1] : '';
        // Trigger only at start-of-text or after whitespace.
        if (i === 0 || /\s/.test(prev)) {
          const query = text.slice(i + 1, offset);
          // Abort if the query contains whitespace (trigger no longer active).
          if (/\s/.test(query)) {
            closeMenu();
            return;
          }
          triggerStart = i;
          triggerNode = node;
          menuQuery = query;
          if (ch === '@') openMentionMenu(query);
          else openSlashMenu(query);
          return;
        }
        closeMenu();
        return;
      }
      if (/\s/.test(ch)) break;
      i -= 1;
    }
    closeMenu();
  }

  function openMentionMenu(query: string) {
    const agents = get(activeAgents);
    const sess = get(sessionsStore).sessions;
    const all = [...agentMentions(agents), ...sessionMentions(sess)];
    mentionItems = filterMentions(all, query);
    if (mentionItems.length === 0) {
      closeMenu();
      return;
    }
    menuMode = 'mention';
    activeIndex = 0;
    const { x, y } = caretRect();
    menuX = x;
    menuY = y;
  }

  function openSlashMenu(query: string) {
    commandItems = filterCommands(query);
    if (commandItems.length === 0) {
      closeMenu();
      return;
    }
    menuMode = 'slash';
    activeIndex = 0;
    const { x, y } = caretRect();
    menuX = x;
    menuY = y;
  }

  function closeMenu() {
    menuMode = 'none';
    triggerStart = -1;
    triggerNode = null;
  }

  /** Replace the trigger token (`@que` / `/res`) with the given plain text and place caret after. */
  function replaceTrigger(replacement: string) {
    if (triggerNode == null || triggerStart < 0) return;
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0) return;
    const offset = sel.getRangeAt(0).startOffset;
    const text = triggerNode.textContent ?? '';
    const before = text.slice(0, triggerStart);
    const after = text.slice(offset);
    const next = before + replacement + after;
    triggerNode.textContent = next;
    // Restore caret right after the inserted replacement.
    const caretPos = before.length + replacement.length;
    const range = document.createRange();
    range.setStart(triggerNode, Math.min(caretPos, (triggerNode.textContent ?? '').length));
    range.collapse(true);
    sel.removeAllRanges();
    sel.addRange(range);
    closeMenu();
    syncValue();
  }

  function selectMention(item: MentionItem) {
    replaceTrigger(flattenMention(item) + ' ');
  }

  function selectCommand(cmd: SlashCommand) {
    if (cmd.action === 'clear') {
      editor.textContent = '';
      closeMenu();
      syncValue();
      return;
    }
    // 'insert' and 'attach' both insert the expansion (attach is a no-op placeholder).
    replaceTrigger(cmd.expand());
  }

  async function submit() {
    // No submit handler (dialog usage with bind:value): never submit, clear, or consume
    // the one-shot context — that would wipe the field and silently eat pending context.
    if (disabled || !onsubmit) return;
    let text = currentText().trim();
    // Prepend one-shot operator context exactly once.
    if (sessionId) {
      const ctx = await pendingContext.consume(sessionId);
      if (ctx) {
        const block = pendingContext.render(ctx);
        text = text ? `${block}\n\n${text}` : block;
      }
    }
    if (!text) return;
    onsubmit?.(text);
    // Clear editor + draft.
    editor.textContent = '';
    value = '';
    if (persistDraft && sessionId) composerDraft.clear();
    closeMenu();
  }

  function moveActive(delta: number) {
    const len = menuMode === 'mention' ? mentionItems.length : commandItems.length;
    if (len === 0) return;
    activeIndex = (activeIndex + delta + len) % len;
  }

  function commitActive() {
    if (menuMode === 'mention') selectMention(mentionItems[activeIndex]);
    else if (menuMode === 'slash') selectCommand(commandItems[activeIndex]);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (menuMode !== 'none') {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        moveActive(1);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        moveActive(-1);
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        commitActive();
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        closeMenu();
        return;
      }
    }

    // Enter submits ONLY when this composer has a submit handler (e.g. the chat input).
    // In dialog usage (bind:value, no onsubmit) Enter inserts a newline like a textarea
    // instead of clearing the field. Shift+Enter always inserts a newline.
    if (e.key === 'Enter' && !e.shiftKey && onsubmit) {
      e.preventDefault();
      submit();
    }
  }

  function handleInput() {
    syncValue();
    detectTrigger();
  }

  async function handleKeyup(e: KeyboardEvent) {
    // Arrow/navigation keys can move the caret across triggers; re-detect after.
    if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key)) {
      await tick();
      detectTrigger();
    }
  }
</script>

<div class="composer-wrap" class:disabled>
  <div
    bind:this={editor}
    class="composer"
    data-composer
    contenteditable="true"
    role="textbox"
    tabindex="0"
    aria-multiline="true"
    aria-label={placeholder}
    data-placeholder={placeholder}
    oninput={handleInput}
    onkeydown={handleKeydown}
    onkeyup={handleKeyup}
    onblur={closeMenu}
  ></div>

  {#if menuMode === 'mention'}
    <MentionMenu
      items={mentionItems}
      {activeIndex}
      x={menuX}
      y={menuY}
      onselect={selectMention}
    />
  {:else if menuMode === 'slash'}
    <SlashMenu
      items={commandItems}
      {activeIndex}
      x={menuX}
      y={menuY}
      onselect={selectCommand}
    />
  {/if}
</div>

<style>
  .composer-wrap {
    position: relative;
    flex: 1;
    min-width: 0;
  }

  .composer {
    width: 100%;
    min-height: 34px;
    max-height: 160px;
    overflow-y: auto;
    padding: 7px 10px;
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-size: 12px;
    font-family: var(--font-mono);
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    outline: none;
  }

  .composer:focus {
    border-color: var(--accent-cyan);
  }

  .composer:empty::before {
    content: attr(data-placeholder);
    color: var(--text-secondary);
    pointer-events: none;
  }

  .composer-wrap.disabled .composer {
    opacity: 0.5;
    pointer-events: none;
  }
</style>
