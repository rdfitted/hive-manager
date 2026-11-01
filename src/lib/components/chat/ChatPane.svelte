<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { tick } from 'svelte';
  import { agentEvents, type AgentEvent } from '$lib/stores/agentEvents';
  import Composer from '../composer/Composer.svelte';
  import ToolRenderHost from '../renderers/ToolRenderHost.svelte';
  import { CHAT_WINDOW_SIZE, formatRaw, passiveToolResult, transcriptRows } from './transcript';

  let { sessionId, agentId, events, raw = false, visible = true }: {
    sessionId: string; agentId: string; events: AgentEvent[]; raw?: boolean; visible?: boolean;
  } = $props();
  const history = $derived($agentEvents.sessionId === sessionId ? $agentEvents.histories[agentId] : undefined);
  const rows = $derived(transcriptRows(events));
  const count = $derived(raw ? events.length : rows.length);
  let end = $state<number | null>(null);
  const stop = $derived(Math.min(end ?? count, count));
  const start = $derived(Math.max(0, stop - CHAT_WINDOW_SIZE));
  let container: HTMLDivElement;
  let atBottom = $state(true);
  let sending = $state(false);
  let error = $state('');

  $effect(() => {
    void agentId; void sessionId; void raw;
    end = null;
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
  async function send(text: string) {
    if (!text.trim() || sending) return;
    sending = true; error = '';
    try {
      await invoke('operator_inject', { request: { session_id: sessionId, target_agent_id: agentId, message: text.trim() } });
    } catch (cause) { error = String(cause); }
    finally { sending = false; }
  }
</script>

<section class="chat-pane lattice-forced-colors-boundary" aria-label={raw ? 'Raw agent event log' : 'Agent chat'}>
  <nav class="history-controls" aria-label="Transcript history">
    <button type="button" class="lattice-btn lattice-btn--ghost" disabled={history?.loading || (start === 0 && !history?.hasMore)} onclick={older}>Older</button>
    <span>{count === 0 ? 'No events yet' : `${start + 1}–${stop} of ${count}`}</span>
    <button type="button" class="lattice-btn lattice-btn--ghost" disabled={stop >= count} onclick={() => { end = Math.min(count, stop + CHAT_WINDOW_SIZE); }}>Newer</button>
    {#if end !== null}<button type="button" class="lattice-btn lattice-btn--ghost" onclick={() => { end = null; atBottom = true; }}>Latest</button>{/if}
  </nav>
  {#if history?.error}<p class="feed-error" role="alert">{history.error}</p>{/if}
  <div class="transcript lattice-scroll-content" bind:this={container} onscroll={() => { atBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 40; }}>
    {#if raw}
      {#each events.slice(start, stop) as event (event.cursor)}
        <article class="raw-event" aria-label={event.kind}><span>{event.kind}</span><pre>{formatRaw(event)}</pre></article>
      {/each}
    {:else}
      {#each rows.slice(start, stop) as row (row.id)}
        {#if row.type === 'tool'}
          <details class="tool-call lattice-forced-colors-boundary">
            <summary>{row.name} · {row.failed ? 'Failed' : row.complete ? 'Complete' : 'Running'}</summary>
            {#if row.input !== undefined}<pre aria-label="Tool input">{formatRaw(row.input)}</pre>{/if}
            {#if row.result !== undefined}
              {@const passive = passiveToolResult(row.result)}
              {#if passive}
                <ToolRenderHost message={{ id: row.id, timestamp: row.ts, from: 'tool', content: '', renderer: passive.renderer, data: passive.data }} />
              {:else}<pre aria-label="Tool result">{formatRaw(row.result)}</pre>{/if}
            {/if}
            {#if row.truncated}<span class="truncation">Output truncated</span>{/if}
          </details>
        {:else}
          <article class="message" aria-label={row.type === 'user' ? 'User message' : row.type === 'assistant' ? 'Assistant message' : 'Agent error'}>
            <header><strong>{row.type === 'user' ? 'User' : row.type === 'assistant' ? 'Assistant' : 'Error'}</strong><time datetime={row.ts}>{new Date(row.ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time></header>
            <p>{row.text}</p>
            {#if row.truncated}<span class="truncation">Message truncated</span>{/if}
          </article>
        {/if}
      {/each}
      {#if count === 0}<p class="empty-chat">Waiting for this agent’s messages.</p>{/if}
    {/if}
  </div>
  {#if error}<p class="feed-error" role="alert">Message could not be sent: {error}</p>{/if}
  <div class="chat-composer"><Composer {sessionId} {agentId} persistDraft={false} disabled={sending || !visible} onsubmit={send} placeholder="Message this agent…" /></div>
</section>

<style>
  .chat-pane { display: flex; flex-direction: column; height: 100%; min-height: 0; min-width: 0; color: var(--text-primary); background: var(--bg-surface); }
  .history-controls { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); padding: var(--space-2); font-size: var(--text-small); color: var(--text-secondary); }
  .transcript { flex: 1; min-height: 0; overflow: auto; padding: var(--space-3); overflow-wrap: anywhere; }
  .message { padding: var(--space-2) 0 var(--space-4); }
  .message header { display: flex; gap: var(--space-3); align-items: baseline; font-size: var(--text-small); }
  .message time, .truncation { color: var(--text-secondary); font-size: var(--text-small); }
  .message p { margin: var(--space-2) 0 0; white-space: pre-wrap; }
  .tool-call { margin-bottom: var(--space-3); padding: var(--space-2); box-shadow: var(--edge-seam); }
  .tool-call summary { cursor: pointer; font-size: var(--text-small); }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; font-family: var(--font-mono); font-size: var(--text-small); }
  .chat-composer { flex: 0 0 auto; padding: var(--space-2); }
  .feed-error { margin: 0; padding: var(--space-2); color: var(--status-error); }
  .empty-chat { color: var(--text-secondary); }
</style>
