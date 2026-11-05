<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { tick } from 'svelte';
  import { agentEvents, type AgentEvent } from '$lib/stores/agentEvents';
  import { activeAgents } from '$lib/stores/sessions';
  import Composer from '../composer/Composer.svelte';
  import ComposerToolbar from '../composer/ComposerToolbar.svelte';
  import { toolbarFacts } from '../composer/toolbarFacts';
  import ActivityGroup from './ActivityGroup.svelte';
  import MarkdownBlocks from '../ui/MarkdownBlocks.svelte';
  import { CHAT_WINDOW_SIZE, formatRaw, liveActivityState, transcriptRows } from './transcript';

  let { sessionId, agentId, events, raw = false, visible = true, composer = true, paging = true, variant = 'focus', draft = $bindable('') }: {
    sessionId: string; agentId: string; events: AgentEvent[]; raw?: boolean; visible?: boolean;
    composer?: boolean; paging?: boolean; variant?: 'focus' | 'column'; draft?: string;
  } = $props();
  const history = $derived($agentEvents.sessionId === sessionId ? $agentEvents.histories[agentId] : undefined);
  const rows = $derived(transcriptRows(events));
  const agent = $derived($activeAgents.find((candidate) => candidate.id === agentId));
  const facts = $derived(toolbarFacts(agent, $agentEvents.sessionId === sessionId ? $agentEvents.feeds?.[agentId]?.capabilities.event_source : undefined));
  let now = $state(Date.now());
  const activity = $derived(liveActivityState(events, agent?.status, now));
  const count = $derived(raw ? events.length : rows.length);
  let end = $state<number | null>(null);
  const stop = $derived(Math.min(end ?? count, count));
  const start = $derived(Math.max(0, stop - CHAT_WINDOW_SIZE));
  let container: HTMLDivElement;
  let atBottom = $state(true);
  let sending = $state(false);
  let error = $state('');
  let expandedTools = $state<Record<string, boolean>>({});
  let expandedMessages = $state<Record<string, boolean>>({});
  let overflowingMessages = $state<Record<string, boolean>>({});
  const canMeasure = typeof ResizeObserver !== 'undefined';
  let lastView = $state('');

  $effect(() => {
    const key = JSON.stringify([sessionId, agentId, raw]);
    if (key !== lastView) { lastView = key; end = null; expandedTools = {}; expandedMessages = {}; overflowingMessages = {}; }
  });
  $effect(() => {
    if (!visible || raw || agent?.status !== 'Running') return;
    now = Date.now();
    const timer = setInterval(() => { now = Date.now(); }, 1000);
    return () => clearInterval(timer);
  });
  $effect(() => {
    void events; void visible;
    if (visible && atBottom && end === null) void tick().then(() => {
      if (container) container.scrollTop = container.scrollHeight;
    });
  });

  async function older() {
    if (start > 0) { end = start; return; }
    const previous = count;
    await agentEvents.loadOlder(agentId);
    await tick();
    end = Math.max(CHAT_WINDOW_SIZE, count - previous);
  }
  function prepareSubmit() {
    const sid = sessionId, id = agentId, original = draft;
    return { send: (text: string) => { void send(text, sid, id); }, clear: () => { if (sessionId === sid && agentId === id && draft === original) draft = ''; } };
  }
  async function send(text: string, sid = sessionId, id = agentId) {
    if (!text.trim() || sending) return;
    sending = true; error = '';
    try {
      await invoke('operator_inject', { request: { session_id: sid, target_agent_id: id, message: text.trim() } });
    } catch (cause) { error = String(cause); }
    finally { sending = false; }
  }
  function scroll() {
    atBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 40;
    if (!atBottom && end === null) end = count;
  }
  function measureOverflow(node: HTMLParagraphElement, id: string | undefined) {
    if (!id || !canMeasure) return;
    const measure = () => {
      if (!expandedMessages[id]) overflowingMessages[id] = node.scrollHeight > node.clientHeight;
    };
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    void tick().then(measure);
    return { destroy() { observer.disconnect(); } };
  }
</script>

<section class="chat-pane lattice-forced-colors-boundary" data-variant={variant} aria-label={raw ? 'Raw agent event log' : 'Agent chat'}>
  {#if paging && (raw || start > 0 || history?.hasMore || end !== null)}
  <nav class="history-controls" aria-label="Transcript history">
    {#if raw || start > 0 || history?.hasMore}<button type="button" class="lattice-btn lattice-btn--ghost" disabled={history?.loading || (start === 0 && !history?.hasMore)} onclick={older}>Older</button>{/if}
    <span>{count === 0 ? 'No events yet' : `${start + 1}–${stop} of ${count}`}</span>
    {#if raw || stop < count}<button type="button" class="lattice-btn lattice-btn--ghost" disabled={stop >= count} onclick={() => { end = Math.min(count, stop + CHAT_WINDOW_SIZE); }}>Newer</button>{/if}
    {#if end !== null}<button type="button" class="lattice-btn lattice-btn--ghost" onclick={() => { end = null; atBottom = true; }}>Latest</button>{/if}
  </nav>
  {/if}
  {#if history?.error}<p class="feed-error" role="alert">{history.error}</p>{/if}
  <div class="transcript lattice-scroll-content" bind:this={container} onscroll={scroll}>
    {#if raw}
      {#each events.slice(start, stop) as event (event.cursor)}
        <article class="raw-event" aria-label={event.kind}><span>{event.kind}</span><pre>{formatRaw(event)}</pre></article>
      {/each}
    {:else}
      {#each rows.slice(start, stop) as row (row.id)}
        {#if row.type === 'activity'}
          <ActivityGroup group={row} expanded={expandedTools[row.id] ?? false} onchange={(open) => { expandedTools[row.id] = open; }} />
        {:else}
          {@const longMessage = row.text.split('\n').length > 6 || row.text.length > 480}
          <article class="message" class:user-message={row.type === 'user'} aria-label={row.type === 'user' ? 'User message' : row.type === 'assistant' ? 'Assistant message' : row.type === 'decision' ? 'Approval decision' : 'Agent error'} title={new Date(row.ts).toLocaleString()}>
            {#if row.type === 'decision' || row.type === 'error'}<strong>{row.type === 'decision' ? 'Approval' : 'Error'}</strong>{/if}
            {#if row.type === 'assistant'}
              <MarkdownBlocks source={row.text} allowHtmlPreview />
            {:else}
              <p use:measureOverflow={row.type === 'user' ? row.id : undefined} class:clamped={row.type === 'user' && !expandedMessages[row.id] && (canMeasure || longMessage)}>{row.text}</p>
            {/if}
            {#if row.type === 'user' && (longMessage || overflowingMessages[row.id])}
              <button type="button" class="lattice-btn lattice-btn--ghost" aria-expanded={expandedMessages[row.id] ?? false} onclick={() => { expandedMessages[row.id] = !expandedMessages[row.id]; }}>{expandedMessages[row.id] ? 'Show less' : 'Show more'}</button>
            {/if}
            {#if row.truncated}<span class="truncation">Message truncated</span>{/if}
          </article>
        {/if}
      {/each}
      {#if count === 0}<p class="empty-chat">Waiting for this agent’s messages.</p>{/if}
      {#if activity && end === null}
        <div class="live-activity">
          <p role="status" class:visually-hidden={activity.label !== activity.status}>{activity.status}</p>
          {#if activity.label !== activity.status}<p aria-hidden="true">{activity.label}</p>{/if}
        </div>
      {/if}
    {/if}
  </div>
  {#if error}<p class="feed-error" role="alert">Message could not be sent: {error}</p>{/if}
  {#if composer}
  <div class="chat-composer"><Composer {sessionId} {agentId} {prepareSubmit} submissionKey={`${sessionId}:${agentId}`} bind:value={draft} persistDraft={false} disabled={sending || !visible} onsubmit={send} placeholder={facts.placeholder}>
    {#snippet toolbar(controls)}{#if visible}<ComposerToolbar {controls} accessLabel={facts.accessLabel} model={facts.model} effort={facts.effort} {sending} />{/if}{/snippet}
  </Composer></div>
  {/if}
</section>

<style>
  .chat-pane { display: flex; flex-direction: column; height: 100%; min-height: 0; min-width: 0; color: var(--text-primary); background: var(--surface); }
  .history-controls { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); padding: var(--space-2); font-size: var(--text-small); color: var(--text-muted); }
  .transcript { flex: 1; min-height: 0; overflow: auto; padding: var(--space-3); overflow-wrap: anywhere; }
  .message { min-width: 0; padding: var(--space-2) 0 var(--space-4); }
  .user-message { margin: var(--space-2) 0 var(--space-4) auto; width: fit-content; max-width: 85%; padding: var(--space-3); border-radius: var(--radius-shell); background: var(--surface-overlay); }
  .message p { margin: 0; white-space: pre-wrap; }
  .message .clamped { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 6; line-clamp: 6; overflow: hidden; }
  .truncation, .live-activity { color: var(--text-muted); font-size: var(--text-small); }
  .visually-hidden { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; border: 0; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; font-family: var(--font-mono); font-size: var(--text-small); }
  .chat-composer { flex: 0 0 auto; padding: var(--space-2); }
  .feed-error { margin: 0; padding: var(--space-2); color: var(--status-error); }
  .empty-chat { color: var(--text-muted); }
  @media (forced-colors: active) { .user-message { border: 1px solid CanvasText; } .live-activity { color: CanvasText; } }
</style>
