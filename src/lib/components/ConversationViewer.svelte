<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { conversationStore, type ConversationMessage } from '$lib/stores/conversations';
  import { activeSession } from '$lib/stores/sessions';
  import AgentStatusBar from './AgentStatusBar.svelte';

  let messageContainer: HTMLDivElement;
  let autoScroll = true;
  let searchQuery = '';
  let messageInput = '';
  let lastMessageCount = 0;
  let pollInterval: ReturnType<typeof setInterval>;

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
        conversationStore.loadConversation(sessionId, selectedAgent);
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

  async function sendMessage() {
    if (!sessionId || !selectedAgent || !messageInput.trim()) return;
    await conversationStore.sendMessage(sessionId, selectedAgent, 'operator', messageInput.trim());
    messageInput = '';
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  }

  function getSenderColor(from: string): string {
    if (from === 'queen') return '#c084fc';
    if (from === 'system' || from === 'SYSTEM') return '#9ca3af';
    if (from === 'operator') return '#7aa2f7';
    if (from.startsWith('worker')) return '#22d3ee';
    return '#c0caf5';
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

<div class="conversation-viewer">
  <AgentStatusBar />

  <!-- Agent tabs -->
  <div class="agent-tabs">
    {#each agentTabs as tab (tab.id)}
      <button
        class="agent-tab"
        class:active={selectedAgent === tab.id}
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
      class="search-input"
    />
    {#if !autoScroll}
      <button class="scroll-btn" onclick={scrollToBottom} title="Scroll to bottom">
        &#8595;
      </button>
    {/if}
  </div>

  <!-- Messages -->
  <div class="messages" bind:this={messageContainer} onscroll={handleScroll}>
    {#if !selectedAgent}
      <div class="empty">Select an agent tab to view conversation.</div>
    {:else if $conversationStore.loading}
      <div class="empty">Loading...</div>
    {:else if filteredMessages.length === 0}
      <div class="empty">
        {#if searchQuery}
          No messages matching "{searchQuery}"
        {:else}
          No messages yet.
        {/if}
      </div>
    {:else}
      {#each filteredMessages as msg, i (i)}
        <div class="message">
          <span class="msg-time">{formatTimestamp(msg.timestamp)}</span>
          <span class="msg-sender" style="color: {getSenderColor(msg.from)}">{msg.from}</span>
          <span class="msg-content">{msg.content}</span>
        </div>
      {/each}
    {/if}
  </div>

  <!-- Input -->
  {#if selectedAgent && selectedAgent !== 'shared'}
    <div class="input-bar">
      <input
        type="text"
        placeholder="Send message as operator..."
        bind:value={messageInput}
        onkeydown={handleKeydown}
        class="message-input"
      />
      <button class="send-btn" onclick={sendMessage} disabled={!messageInput.trim()}>
        Send
      </button>
    </div>
  {/if}

  {#if $conversationStore.error}
    <div class="error">
      {$conversationStore.error}
      <button class="dismiss-btn" onclick={() => conversationStore.clearError()}>&#10005;</button>
    </div>
  {/if}
</div>

<style>
  .conversation-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-secondary, #1a1b26);
  }

  .agent-tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 0;
    background: var(--bg-tertiary, #24283b);
    border-bottom: 1px solid var(--border-color, #414868);
    padding: 0 4px;
    overflow-x: auto;
  }

  .agent-tab {
    padding: 8px 12px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary, #565f89);
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    cursor: pointer;
    white-space: nowrap;
    transition: color 0.15s, border-color 0.15s;
  }

  .agent-tab:hover {
    color: var(--text-primary, #c0caf5);
  }

  .agent-tab.active {
    color: var(--accent-color, #7aa2f7);
    border-bottom-color: var(--accent-color, #7aa2f7);
  }

  .controls {
    display: flex;
    gap: 8px;
    padding: 8px 12px;
    align-items: center;
  }

  .search-input {
    flex: 1;
    padding: 4px 8px;
    font-size: 12px;
    background: var(--bg-primary, #1a1b26);
    border: 1px solid var(--border-color, #414868);
    border-radius: 4px;
    color: var(--text-primary, #c0caf5);
  }

  .search-input:focus {
    outline: none;
    border-color: var(--accent-color, #7aa2f7);
  }

  .scroll-btn {
    padding: 4px 8px;
    font-size: 12px;
    background: var(--accent-color, #7aa2f7);
    border: none;
    border-radius: 4px;
    color: white;
    cursor: pointer;
  }

  .messages {
    flex: 1;
    overflow-y: auto;
    padding: 8px 12px;
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
    font-size: 12px;
    line-height: 1.6;
  }

  .empty {
    color: var(--text-secondary, #565f89);
    text-align: center;
    padding: 24px;
    font-style: italic;
  }

  .message {
    display: flex;
    gap: 6px;
    padding: 3px 0;
    border-bottom: 1px solid var(--border-color-subtle, #292e42);
  }

  .message:last-child {
    border-bottom: none;
  }

  .msg-time {
    color: var(--text-muted, #3b4261);
    font-size: 11px;
    flex-shrink: 0;
  }

  .msg-sender {
    font-weight: 600;
    flex-shrink: 0;
  }

  .msg-content {
    color: var(--text-primary, #c0caf5);
    word-break: break-word;
  }

  .input-bar {
    display: flex;
    gap: 8px;
    padding: 8px 12px;
    border-top: 1px solid var(--border-color, #414868);
    background: var(--bg-tertiary, #24283b);
  }

  .message-input {
    flex: 1;
    padding: 6px 10px;
    font-size: 12px;
    background: var(--bg-primary, #1a1b26);
    border: 1px solid var(--border-color, #414868);
    border-radius: 4px;
    color: var(--text-primary, #c0caf5);
  }

  .message-input:focus {
    outline: none;
    border-color: var(--accent-color, #7aa2f7);
  }

  .send-btn {
    padding: 6px 14px;
    font-size: 12px;
    font-weight: 600;
    background: var(--accent-color, #7aa2f7);
    border: none;
    border-radius: 4px;
    color: white;
    cursor: pointer;
  }

  .send-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .send-btn:hover:not(:disabled) {
    opacity: 0.9;
  }

  .error {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    background: var(--error-bg, #3b2030);
    color: var(--error-text, #f7768e);
    font-size: 12px;
  }

  .dismiss-btn {
    background: none;
    border: none;
    color: var(--error-text, #f7768e);
    cursor: pointer;
    padding: 2px 6px;
  }
</style>
