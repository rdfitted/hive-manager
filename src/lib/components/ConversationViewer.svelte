<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { conversationStore, type ConversationMessage } from '$lib/stores/conversations';
  import { activeSession } from '$lib/stores/sessions';
  import AgentStatusBar from './AgentStatusBar.svelte';
  import ToolRenderHost from './renderers/ToolRenderHost.svelte';
  import Composer from './composer/Composer.svelte';
  import Skeleton from './Skeleton.svelte';
  import SkelBar from './SkelBar.svelte';

  let messageContainer: HTMLDivElement;
  let autoScroll = true;
  let searchQuery = '';
  let messageInput = '';
  let lastMessageCount = 0;
  let pollInterval: ReturnType<typeof setInterval>;
  let approvalError = '';

  $: sessionId = $activeSession?.id ?? null;
  $: agents = $activeSession?.agents ?? [];
  $: selectedAgent = $conversationStore.selectedAgent;

  // Build tab list: queen, workers, shared
  $: agentTabs = buildTabs(agents);

  function buildTabs(agentList: typeof agents): { id: string; label: string }[] {
    const tabs: { id: string; label: string }[] = [];
    for (const a of agentList) {
      const role = a.role;
      let label = a.config?.label || a.id.slice(0, 8);
      if (role === 'Queen') label = 'Queen';
      else if (role === 'MasterPlanner') label = 'Planner';
      else if (typeof role === 'object' && role !== null) {
        if ('Worker' in (role as Record<string, unknown>)) {
          const w = (role as { Worker: { index: number } }).Worker;
          label = a.config?.label || `Worker ${w.index}`;
        }
      }
      tabs.push({ id: a.id, label });
    }
    tabs.push({ id: 'shared', label: 'Shared' });
    return tabs;
  }

  // Set session on the store when active session changes
  $: if (sessionId) {
    conversationStore.setSessionId(sessionId);
  }

  // Load conversation when agent tab is selected
  function selectTab(agentId: string) {
    if (!sessionId) return;
    conversationStore.selectAgent(agentId);
    conversationStore.loadConversation(sessionId, agentId);
  }

  // Auto-scroll on new messages
  $: {
    const count = $conversationStore.messages.length;
    if (count > lastMessageCount && autoScroll && messageContainer) {
      lastMessageCount = count;
      setTimeout(() => {
        if (messageContainer) messageContainer.scrollTop = messageContainer.scrollHeight;
      }, 0);
    }
  }

  // Poll for new messages
  onMount(() => {
    pollInterval = setInterval(() => {
      if (sessionId && selectedAgent) {
        conversationStore.loadConversation(sessionId, selectedAgent, undefined, { silent: true });
      }
    }, 5000);
  });

  onDestroy(() => {
    clearInterval(pollInterval);
  });

  function handleScroll() {
    if (messageContainer) {
      const isAtBottom =
        messageContainer.scrollHeight - messageContainer.scrollTop <= messageContainer.clientHeight + 50;
      autoScroll = isAtBottom;
    }
  }

  function scrollToBottom() {
    if (messageContainer) {
      messageContainer.scrollTop = messageContainer.scrollHeight;
      autoScroll = true;
    }
  }

  // The Composer flattens its contenteditable model (mentions/commands) to a plain string
  // and prepends any one-shot operator context before handing the final prompt here.
  async function sendComposerMessage(text: string) {
    if (!sessionId || !selectedAgent || !text.trim()) return;
    await conversationStore.sendMessage(sessionId, selectedAgent, 'operator', text.trim());
  }

  function queenAgentId(): string | null {
    const queen = agents.find((agent) => agent.role === 'Queen' || agent.id.endsWith('-queen'));
    return queen?.id ?? null;
  }

  async function injectApprovalDecision(decision: 'approved' | 'rejected', detail: { actionId?: string }) {
    if (!sessionId) return;
    const targetAgentId = queenAgentId() ?? (selectedAgent !== 'shared' ? selectedAgent : null);
    if (!targetAgentId) {
      approvalError = 'No agent is available for approval injection.';
      return;
    }

    const actionId = detail?.actionId ?? 'unknown';
    const message = [
      `Operator ${decision} approval request.`,
      `action_id: ${actionId}`,
      `decision: ${decision}`,
    ].join('\n');

    try {
      approvalError = '';
      await invoke('operator_inject', {
        request: {
          session_id: sessionId,
          target_agent_id: targetAgentId,
          message,
        },
      });
    } catch (err) {
      approvalError = String(err);
      console.error('[tool-render] approval injection failed', err);
    }
  }

  function handleApprove(detail: { actionId?: string }) {
    void injectApprovalDecision('approved', detail);
  }

  function handleReject(detail: { actionId?: string }) {
    void injectApprovalDecision('rejected', detail);
  }

  function getSenderColor(from: string): string {
    if (from === 'queen') return 'var(--accent-cyan)';
    if (from === 'system' || from === 'SYSTEM') return 'var(--accent-chrome)';
    if (from === 'operator') return 'var(--accent-amber)';
    if (from.startsWith('worker')) return 'var(--accent-amber)';
    return 'var(--text-primary)';
  }

  function formatTimestamp(ts: string): string {
    const date = new Date(ts);
    return date.toLocaleTimeString('en-US', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
  }

  $: filteredMessages = searchQuery.trim()
    ? $conversationStore.messages.filter(
        (m) =>
          m.from.toLowerCase().includes(searchQuery.toLowerCase()) ||
          m.content.toLowerCase().includes(searchQuery.toLowerCase())
      )
    : $conversationStore.messages;
</script>

{#snippet conversationSkeleton()}
  <div class="conversation-skeleton">
    {#each [0, 1, 2] as _}
      <div class="skeleton-message">
        <SkelBar width="4.25rem" height="0.65rem" />
        <SkelBar width="5rem" height="0.65rem" />
        <SkelBar width="min(65%, 26rem)" height="0.65rem" />
      </div>
    {/each}
  </div>
{/snippet}

<div class="conversation-viewer">
  <AgentStatusBar />

  <!-- Agent tabs -->
  <div class="agent-tabs lattice-scroll-chrome lattice-forced-colors-boundary" role="tablist">
    {#each agentTabs as tab (tab.id)}
      <button
        class="lattice-tab"
        class:lattice-tab--active={selectedAgent === tab.id}
        aria-selected={selectedAgent === tab.id}
        role="tab"
        onclick={() => selectTab(tab.id)}
      >
        {tab.label}
      </button>
    {/each}
  </div>

  <!-- Search + controls -->
  <div class="controls">
    <input
      type="text"
      placeholder="Search messages..."
      bind:value={searchQuery}
      class="search-input lattice-input"
    />
    {#if !autoScroll}
      <button class="lattice-btn lattice-btn--ghost lattice-btn--icon" onclick={scrollToBottom} title="Scroll to bottom" aria-label="Scroll to bottom">
        &#8595;
      </button>
    {/if}
  </div>

  <!-- Messages -->
  <div class="messages lattice-scroll-content" bind:this={messageContainer} onscroll={handleScroll}>
    {#if !selectedAgent}
      <div class="empty">Select an agent tab to view conversation.</div>
    {:else}
      <Skeleton loading={$conversationStore.loading} skeleton={conversationSkeleton} class="conversation-loading">
        {#if filteredMessages.length === 0}
          <div class="empty">
            {#if searchQuery}
              No messages matching "{searchQuery}"
            {:else}
              No messages yet.
            {/if}
          </div>
        {:else}
          {#each filteredMessages as msg, i (i)}
            <div class="message lattice-forced-colors-boundary">
              <span class="msg-time">{formatTimestamp(msg.timestamp)}</span>
              <span class="msg-sender" style="color: {getSenderColor(msg.from)}">{msg.from}</span>
              {#if msg.renderer || msg.data}
                <div class="msg-content msg-widget">
                  <ToolRenderHost
                    message={msg}
                    onapprove={handleApprove}
                    onreject={handleReject}
                  />
                </div>
              {:else}
                <span class="msg-content">{msg.content}</span>
              {/if}
            </div>
          {/each}
        {/if}
      </Skeleton>
    {/if}
  </div>

  <!-- Input -->
  {#if selectedAgent && selectedAgent !== 'shared'}
    <div class="input-bar lattice-forced-colors-boundary">
      <Composer
        sessionId={sessionId}
        agentId={selectedAgent}
        placeholder="Send message as operator… (@ to mention, / for commands)"
        bind:value={messageInput}
        onsubmit={sendComposerMessage}
      />
    </div>
  {/if}

  {#if $conversationStore.error}
    <div class="error lattice-forced-colors-boundary">
      {$conversationStore.error}
      <button class="lattice-btn lattice-btn--ghost lattice-btn--danger lattice-btn--icon" onclick={() => conversationStore.clearError()} aria-label="Dismiss conversation error">&#10005;</button>
    </div>
  {/if}

  {#if approvalError}
    <div class="error lattice-forced-colors-boundary">
      {approvalError}
      <button class="lattice-btn lattice-btn--ghost lattice-btn--danger lattice-btn--icon" onclick={() => approvalError = ''} aria-label="Dismiss approval error">&#10005;</button>
    </div>
  {/if}
</div>

<style>
  .conversation-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-void);
  }

  .agent-tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 0;
    box-shadow: var(--edge-seam);
    padding: 0 4px;
    overflow-x: auto;
  }

  .controls {
    display: flex;
    gap: 8px;
    padding: 8px 12px;
    align-items: center;
  }

  .search-input {
    flex: 1;
  }

  .messages {
    flex: 1;
    overflow-y: auto;
    padding: 8px 12px;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.6;
  }

  .empty {
    color: var(--text-secondary);
    text-align: center;
    padding: 24px;
    font-style: italic;
  }

  .conversation-skeleton {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .skeleton-message {
    display: grid;
    grid-template-columns: 4.25rem 5rem minmax(8rem, 1fr);
    align-items: center;
    gap: 6px;
    padding: 3px 0;
  }

  .message {
    display: flex;
    gap: 6px;
    padding: 3px 0;
    box-shadow: var(--edge-seam);
  }

  .message:last-child {
    box-shadow: none;
  }

  .msg-time {
    color: var(--text-secondary);
    font-size: 11px;
    flex-shrink: 0;
  }

  .msg-sender {
    font-weight: 600;
    flex-shrink: 0;
  }

  .msg-content {
    color: var(--text-primary);
    word-break: break-word;
  }

  .msg-widget {
    flex: 1;
    min-width: 0;
  }

  .input-bar {
    display: flex;
    gap: 8px;
    padding: 8px 12px;
    box-shadow: var(--edge-seam-top);
    /* Composer dock remains a distinct full-width input surface. */
    background: var(--bg-surface);
  }

  .error {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    /* Full-width semantic error strip intentionally remains state-adjacent chrome. */
    background: var(--bg-surface);
    color: var(--status-error);
    font-size: 12px;
    box-shadow: inset 0 0 0 1px var(--status-error);
  }

</style>
