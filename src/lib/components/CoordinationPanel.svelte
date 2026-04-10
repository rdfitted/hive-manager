<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { coordination, type CoordinationMessage } from '$lib/stores/coordination';
  import { activeSession } from '$lib/stores/sessions';

  let logContainer: HTMLDivElement;
  let autoScroll = true;
  let searchQuery = '';
  let lastLoadedSessionId: string | null = null;
  let lastLogLength = 0;

  // Load coordination log when session changes (using simple reactive check)
  $: {
    const sessionId = $activeSession?.id;
    if (sessionId && sessionId !== lastLoadedSessionId) {
      lastLoadedSessionId = sessionId;
      coordination.loadLog(sessionId);
    }
  }

  // Auto-scroll to bottom when new messages arrive (non-mutating check)
  $: {
    const logLength = $coordination.log.length;
    if (logLength > lastLogLength && autoScroll && logContainer) {
      lastLogLength = logLength;
      setTimeout(() => {
        logContainer.scrollTop = logContainer.scrollHeight;
      }, 0);
    }
  }

  function handleScroll() {
    if (logContainer) {
      const isAtBottom =
        logContainer.scrollHeight - logContainer.scrollTop <= logContainer.clientHeight + 50;
      autoScroll = isAtBottom;
    }
  }

  function scrollToBottom() {
    if (logContainer) {
      logContainer.scrollTop = logContainer.scrollHeight;
      autoScroll = true;
    }
  }

  function getSenderColor(from: string): string {
    if (from === 'QUEEN') return 'text-purple-400';
    if (from === 'SYSTEM') return 'text-gray-400';
    if (from.startsWith('WORKER')) return 'text-cyan-400';
    if (from.startsWith('PLANNER')) return 'text-yellow-400';
    return 'text-gray-300';
  }

  function getSenderIcon(from: string): string {
    if (from === 'QUEEN') return '\u2655'; // Queen chess piece
    if (from === 'SYSTEM') return '\u2699'; // Gear
    if (from.startsWith('WORKER')) return '\u25CF'; // Filled circle
    if (from.startsWith('PLANNER')) return '\u25C6'; // Diamond
    return '\u25CB'; // Empty circle
  }

  function formatTimestamp(ts: string): string {
    const date = new Date(ts);
    return date.toLocaleTimeString('en-US', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
  }

  function filteredMessages(messages: CoordinationMessage[], query: string): CoordinationMessage[] {
    if (!query.trim()) return messages;
    const lower = query.toLowerCase();
    return messages.filter(
      (m) =>
        m.from.toLowerCase().includes(lower) ||
        m.to.toLowerCase().includes(lower) ||
        m.content.toLowerCase().includes(lower)
    );
  }

  $: displayMessages = filteredMessages($coordination.log, searchQuery);
</script>

<div class="coordination-panel">
  <div class="panel-header">
    <h3>Coordination Log</h3>
    <div class="header-actions">
      <input
        type="text"
        placeholder="Search..."
        bind:value={searchQuery}
        class="search-input"
      />
      {#if !autoScroll}
        <button class="scroll-btn" on:click={scrollToBottom} title="Scroll to bottom">
          \u2193
        </button>
      {/if}
    </div>
  </div>

  <div class="log-container" bind:this={logContainer} on:scroll={handleScroll}>
    {#if $coordination.loading}
      <div class="loading">Loading coordination log...</div>
    {:else if displayMessages.length === 0}
      <div class="empty">
        {#if searchQuery}
          No messages matching "{searchQuery}"
        {:else}
          No coordination messages yet.
        {/if}
      </div>
    {:else}
      {#each displayMessages as message (message.id)}
        <div class="message">
          <span class="timestamp">{formatTimestamp(message.timestamp)}</span>
          <span class="sender {getSenderColor(message.from)}">
            <span class="sender-icon">{getSenderIcon(message.from)}</span>
            {message.from}
          </span>
          <span class="arrow">\u2192</span>
          <span class="recipient">{message.to}</span>
          <span class="colon">:</span>
          <span class="content">{message.content}</span>
        </div>
      {/each}
    {/if}
  </div>

  {#if $coordination.error}
    <div class="error">
      {$coordination.error}
      <button class="dismiss-btn" on:click={() => coordination.clearError()}>\u2715</button>
    </div>
  {/if}
</div>

<style>
  .coordination-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-void);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border-structural);
  }

  .panel-header h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .header-actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .search-input {
    padding: 4px 8px;
    font-size: 12px;
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    width: 120px;
  }

  .search-input:focus {
    outline: none;
    border-color: var(--accent-cyan);
  }

  .scroll-btn {
    padding: 4px 8px;
    font-size: 12px;
    background: var(--accent-cyan);
    border: none;
    border-radius: var(--radius-sm);
    color: white;
    cursor: pointer;
  }

  .scroll-btn:hover {
    opacity: 0.9;
  }

  .log-container {
    flex: 1;
    overflow-y: auto;
    padding: 8px 12px;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.6;
  }

  .loading,
  .empty {
    color: var(--text-secondary);
    text-align: center;
    padding: 24px;
    font-style: italic;
  }

  .message {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 4px 0;
    border-bottom: 1px solid var(--border-structural);
  }

  .message:last-child {
    border-bottom: none;
  }

  .timestamp {
    color: var(--text-secondary);
    font-size: 11px;
  }

  .sender {
    font-weight: 600;
  }

  .sender-icon {
    margin-right: 2px;
  }

  .arrow {
    color: var(--text-secondary);
    margin: 0 2px;
  }

  .recipient {
    color: var(--text-secondary);
  }

  .colon {
    color: var(--text-secondary);
  }

  .content {
    color: var(--text-primary);
    flex: 1;
    word-break: break-word;
  }

  .text-purple-400 {
    color: var(--accent-cyan);
  }

  .text-cyan-400 {
    color: var(--accent-cyan);
  }

  .text-yellow-400 {
    color: var(--status-warning);
  }

  .text-gray-400 {
    color: var(--text-secondary);
  }

  .text-gray-300 {
    color: var(--text-secondary);
  }

  .error {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    background: var(--bg-elevated);
    color: var(--status-error);
    font-size: 12px;
  }

  .dismiss-btn {
    background: none;
    border: none;
    color: var(--status-error);
    cursor: pointer;
    padding: 2px 6px;
  }

  .dismiss-btn:hover {
    opacity: 0.8;
  }
</style>
