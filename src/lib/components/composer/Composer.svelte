<script module lang="ts">
  export type ComposerControls = {
    canSend: boolean;
    submit(): void;
    openMenu(mode: 'mention' | 'slash'): void;
  };
</script>

<script lang="ts">
  import { onMount, tick, type Snippet } from 'svelte';
  import { get } from 'svelte/store';
  import MentionMenu from './MentionMenu.svelte';
  import SlashMenu from './SlashMenu.svelte';
  import {
    agentMentions,
    sessionMentions,
    fileMentions,
    listSessionFiles,
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
    toolbar?: Snippet<[ComposerControls]>;
  }

  let {
    sessionId,
    agentId = null,
    placeholder = 'Message…',
    value = $bindable(''),
    persistDraft = true,
    onsubmit,
    disabled = false,
    toolbar,
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
  let mentionRequest = 0;
  let submitting = $state(false);

  let mentionItems: MentionItem[] = $state([]);
  let commandItems: SlashCommand[] = $state([]);
  const controls: ComposerControls = {
    get canSend() { return !disabled && !submitting && !!onsubmit && (!!value.trim() || (!!sessionId && $pendingContext.current?.sessionId === sessionId)); },
    submit,
    openMenu,
  };

  function focusCard(event: PointerEvent) {
    if (disabled || !(event.target instanceof Element)) return;
    if (event.target.closest('[data-composer], button, input, select, textarea, a, [role="menu"], [role="listbox"]')) return;
    event.preventDefault();
    editor.focus();
  }

  /** Insert a real trigger so toolbar actions share keyboard menus and token replacement. */
  function openMenu(mode: 'mention' | 'slash') {
    if (disabled) return;
    editor.focus();
    const selection = window.getSelection();
    if (!selection) return;
    let range = selection.rangeCount ? selection.getRangeAt(0) : document.createRange();
    if (!editor.contains(range.commonAncestorContainer)) {
      range = document.createRange();
      range.selectNodeContents(editor);
      range.collapse(false);
    }
    const before = range.cloneRange();
    before.selectNodeContents(editor);
    before.setEnd(range.startContainer, range.startOffset);
    const prefix = /\S$/.test(before.toString()) ? ' ' : '';
    const trigger = document.createTextNode(prefix + (mode === 'mention' ? '@' : '/'));
    range.deleteContents();
    range.insertNode(trigger);
    range.setStart(trigger, trigger.length);
    range.collapse(true);
    selection.removeAllRanges();
    selection.addRange(range);
    syncValue();
    detectTrigger();
  }

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

  function flattenNode(node: Node): string {
    if (node instanceof HTMLElement && node.dataset.tokenValue) {
      return node.dataset.tokenValue;
    }
    if (node.nodeType === Node.TEXT_NODE) {
      return node.textContent ?? '';
    }
    return Array.from(node.childNodes).map(flattenNode).join('');
  }

  function currentText(): string {
    return editor ? Array.from(editor.childNodes).map(flattenNode).join('') : '';
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
          if (ch === '@') void openMentionMenu(query);
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

  async function openMentionMenu(query: string) {
    const request = ++mentionRequest;
    const agents = get(activeAgents);
    const storeState = get(sessionsStore);
    const sess = storeState.sessions;
    let all = [...agentMentions(agents), ...sessionMentions(sess)];
    if (sessionId) {
      const files = await listSessionFiles(sessionId, query);
      if (request !== mentionRequest || triggerNode == null) return;
      all = [...all, ...fileMentions(files)];
    }
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
    mentionRequest += 1;
    menuMode = 'none';
    triggerStart = -1;
    triggerNode = null;
  }

  function tokenDisplayForMention(item: MentionItem): string {
    if (item.kind === 'agent') return `@${item.label}`;
    if (item.kind === 'session') return `#${item.label}`;
    return `file:${item.label}`;
  }

  function replaceTriggerWithToken(display: string, value: string, kind: 'mention' | 'command') {
    if (triggerNode == null || triggerStart < 0) return;
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0) return;
    const offset = sel.getRangeAt(0).startOffset;
    const text = triggerNode.textContent ?? '';
    const beforeText = text.slice(0, triggerStart);
    const afterText = text.slice(offset);
    const parent = triggerNode.parentNode;
    if (!parent) return;

    const before = document.createTextNode(beforeText);
    const token = document.createElement('span');
    token.className = `composer-token lattice-forced-colors-boundary ${kind}`;
    token.dataset.tokenValue = value;
    token.contentEditable = 'false';
    token.textContent = display;
    const spacer = document.createTextNode(' ');
    const after = document.createTextNode(afterText);

    parent.insertBefore(before, triggerNode);
    parent.insertBefore(token, triggerNode);
    parent.insertBefore(spacer, triggerNode);
    parent.insertBefore(after, triggerNode);
    parent.removeChild(triggerNode);

    const range = document.createRange();
    range.setStartAfter(spacer);
    range.collapse(true);
    sel.removeAllRanges();
    sel.addRange(range);
    closeMenu();
    syncValue();
  }

  function selectMention(item: MentionItem) {
    replaceTriggerWithToken(tokenDisplayForMention(item), flattenMention(item), 'mention');
  }

  function selectCommand(cmd: SlashCommand) {
    if (cmd.action === 'clear') {
      editor.textContent = '';
      closeMenu();
      syncValue();
      return;
    }
    const expanded = cmd.expand().trimEnd();
    replaceTriggerWithToken(cmd.label, expanded || cmd.label, 'command');
  }

  async function submit() {
    // No submit handler (dialog usage with bind:value): never submit, clear, or consume
    // the one-shot context — that would wipe the field and silently eat pending context.
    if (disabled || !onsubmit || submitting) return;
    submitting = true;
    try {
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
    } finally {
      submitting = false;
    }
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

<div class="composer-wrap" role="group" aria-label="Message composer" class:disabled class:has-toolbar={!!toolbar} onpointerdown={focusCard}>
  <div
    bind:this={editor}
    class="composer lattice-scroll-chrome"
    data-composer
    contenteditable="true"
    role="textbox"
    tabindex="0"
    aria-multiline="true"
    aria-disabled={disabled}
    aria-label={placeholder}
    data-placeholder={placeholder}
    oninput={handleInput}
    onkeydown={handleKeydown}
    onkeyup={handleKeyup}
    onblur={closeMenu}
  ></div>

  {#if toolbar}
    <div class="composer-toolbar" role="group" aria-label="Composer toolbar" inert={disabled}>
      {@render toolbar(controls)}
    </div>
  {/if}

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
    border: 1px solid var(--border-control);
    border-radius: var(--radius-shell);
    background: var(--input);
    padding: var(--space-2);
  }

  .composer-wrap:focus-within {
    border-color: var(--accent);
  }

  .composer-wrap.has-toolbar {
    padding: var(--space-3);
  }

  .composer-toolbar {
    padding-top: var(--space-3);
  }

  .composer {
    width: 100%;
    min-height: 34px;
    max-height: 160px;
    overflow-y: auto;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: var(--font-body);
    font-size: var(--text-small);
    color: var(--text-primary);
    outline: none;
  }

  .has-toolbar .composer {
    min-height: 64px;
  }

  .composer :global(.composer-token) {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    margin: 0 2px;
    padding: 1px 6px;
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 45%, var(--border-structural));
    border-radius: var(--radius-sm);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--accent);
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.35;
    vertical-align: baseline;
    white-space: nowrap;
  }

  .composer :global(.composer-token.command) {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 45%, var(--border-structural));
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--accent);
  }

  .composer:empty::before {
    content: attr(data-placeholder);
    color: var(--text-muted);
    pointer-events: none;
  }

  .composer-wrap.disabled .composer {
    opacity: 0.5;
    pointer-events: none;
  }

  @media (forced-colors: active) {
    .composer-wrap { border-color: CanvasText; }
    .composer-wrap:focus-within { border-color: Highlight; }
    .composer:focus-visible { outline: 2px solid Highlight; outline-offset: 2px; }
    .composer :global(.composer-token) { border: 1px solid CanvasText; }
  }
</style>
