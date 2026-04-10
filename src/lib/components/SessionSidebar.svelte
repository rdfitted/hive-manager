<script lang="ts">
  import { CaretDown, CaretRight, Check, ClipboardText, PencilSimple } from 'phosphor-svelte';
  import { sessions, activeSession, serdeEnumVariantName, type Session, type HiveLaunchConfig, type SwarmLaunchConfig, type FusionLaunchConfig, type SoloLaunchConfig } from '$lib/stores/sessions';
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';

  let closingSessionId = $state<string | null>(null);
  let showCloseConfirm = $state<string | null>(null);
  let closing = $state(false);

  function handleCloseSession(e: Event, sessionId: string) {
    e.stopPropagation();
    showCloseConfirm = sessionId;
  }

  function dismissCloseConfirm() {
    if (!closing) {
      showCloseConfirm = null;
    }
  }

  async function confirmCloseSession() {
    const sessionId = showCloseConfirm;
    if (!sessionId) return;

    closing = true;
    closingSessionId = sessionId;

    try {
      await sessions.closeSession(sessionId);
      showCloseConfirm = null;
    } catch (err) {
      console.error('Failed to close session:', err);
    } finally {
      closing = false;
      closingSessionId = null;
    }
  }
  import LaunchDialog from './LaunchDialog.svelte';

  interface Props {
    onLaunch: (projectPath: string, workerCount: number, command: string, prompt?: string) => Promise<void>;
    onLaunchHiveV2?: (config: HiveLaunchConfig) => Promise<void>;
    onLaunchSwarm?: (config: SwarmLaunchConfig) => Promise<void>;
    onLaunchFusion?: (config: FusionLaunchConfig) => Promise<void>;
    onLaunchSolo?: (config: SoloLaunchConfig) => Promise<void>;
  }

  interface SessionSummary {
    id: string;
    session_type: string;
    project_path: string;
    created_at: string;
    agent_count: number;
    state: string;
  }

  let { onLaunch, onLaunchHiveV2, onLaunchSwarm, onLaunchFusion, onLaunchSolo }: Props = $props();

  let showLaunchDialog = $state(false);
  let launching = $state(false);
  let sidebarCollapsed = $state(true);
  let activeCollapsed = $state(false);
  let recentCollapsed = $state(true);
  let persistedSessions = $state<SessionSummary[]>([]);
  let loadingPersisted = $state(false);
  let currentDirectory = $state<string | null>(null);

  onMount(async () => {
    // Get current working directory first
    try {
      currentDirectory = await invoke<string>('get_current_directory');
    } catch (err) {
      console.error('Failed to get current directory:', err);
    }
    await loadPersistedSessions();
  });

  async function loadPersistedSessions() {
    loadingPersisted = true;
    try {
      // Filter by current directory if available
      persistedSessions = await invoke<SessionSummary[]>('list_stored_sessions', {
        projectPath: currentDirectory
      });
    } catch (err) {
      console.error('Failed to load persisted sessions:', err);
    } finally {
      loadingPersisted = false;
    }
  }

  function formatTimestamp(dateStr: string): string {
    const date = new Date(dateStr);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMs / 3600000);
    const diffDays = Math.floor(diffMs / 86400000);

    if (diffMins < 1) return 'Just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    return date.toLocaleDateString();
  }

  function isActiveState(state: Session['state']): boolean {
    if (typeof state === 'object' && state !== null && 'Failed' in state) return false;
    const v = serdeEnumVariantName(state);
    if (v === 'Completed' || v === 'Closed' || v === 'Closing' || v === 'Failed') return false;
    return true;
  }

  function selectSession(sessionId: string) {
    sessions.setActiveSession(sessionId);
  }

  function handleSessionButtonKeydown(event: KeyboardEvent, sessionId: string) {
    const target = event.target;
    if (target instanceof HTMLElement) {
      if (target.tagName === 'INPUT' || target.tagName === 'BUTTON' || target.isContentEditable || target.closest('[contenteditable]')) {
        return;
      }
    }

    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      selectSession(sessionId);
    }
  }

  async function handleResumeSession(sessionId: string) {
    try {
      await sessions.resumeSession(sessionId);
      // Remove from persisted list after successful resume
      persistedSessions = persistedSessions.filter(s => s.id !== sessionId);
    } catch (err) {
      console.error('Failed to resume session:', err);
    }
  }

  async function handleLaunchHive(e: CustomEvent<HiveLaunchConfig>) {
    launching = true;
    try {
      if (onLaunchHiveV2) {
        await onLaunchHiveV2(e.detail);
      } else {
        // Fallback to old launch method
        await onLaunch(
          e.detail.project_path,
          e.detail.workers.length,
          e.detail.queen_config.cli,
          e.detail.prompt
        );
      }
      showLaunchDialog = false;
    } catch (err) {
      console.error('Launch failed:', err);
    } finally {
      launching = false;
    }
  }

  async function handleLaunchSwarm(e: CustomEvent<SwarmLaunchConfig>) {
    launching = true;
    try {
      if (onLaunchSwarm) {
        await onLaunchSwarm(e.detail);
        showLaunchDialog = false;
      } else {
        console.error('Swarm launch not supported');
      }
    } catch (err) {
      console.error('Launch failed:', err);
    } finally {
      launching = false;
    }
  }

  async function handleLaunchFusion(e: CustomEvent<FusionLaunchConfig>) {
    launching = true;
    try {
      if (onLaunchFusion) {
        await onLaunchFusion(e.detail);
        showLaunchDialog = false;
      } else {
        console.error('Fusion launch not supported');
      }
    } catch (err) {
      console.error('Launch failed:', err);
    } finally {
      launching = false;
    }
  }

  async function handleLaunchSolo(e: CustomEvent<SoloLaunchConfig>) {
    launching = true;
    try {
      if (onLaunchSolo) {
        await onLaunchSolo(e.detail);
      } else {
        await sessions.launchSolo(e.detail);
      }
      showLaunchDialog = false;
    } catch (err) {
      console.error('Launch failed:', err);
    } finally {
      launching = false;
    }
  }

  const COLORS = [
    { name: 'Blue', value: '#7aa2f7' },
    { name: 'Purple', value: '#bb9af7' },
    { name: 'Green', value: '#9ece6a' },
    { name: 'Yellow', value: '#e0af68' },
    { name: 'Cyan', value: '#7dcfff' },
    { name: 'Red', value: '#f7768e' },
    { name: 'Orange', value: '#ff9e64' },
    { name: 'Pink', value: '#f7b1d1' },
  ];

  let editingSessionId = $state<string | null>(null);
  let editName = $state('');
  let editColor = $state<string | null | undefined>(undefined);
  let showColorPicker = $state(false);

  function startEdit(session: Session) {
    editingSessionId = session.id;
    editName = session.name || session.project_path.split(/[/\\]/).pop() || '';
    editColor = session.color;
    showColorPicker = false;
  }

  async function saveMetadata() {
    if (!editingSessionId) return;
    try {
      await sessions.updateSessionMetadata(editingSessionId, editName, editColor);
      editingSessionId = null;
    } catch (err) {
      console.error('Failed to update metadata:', err);
    }
  }

  function cancelEdit() {
    editingSessionId = null;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;

    if (e.key === 'Enter') {
      saveMetadata();
    } else if (e.key === 'Escape') {
      cancelEdit();
    }
  }
</script>

<aside class="sidebar" class:collapsed={sidebarCollapsed}>
  <button class="sidebar-header" onclick={() => sidebarCollapsed = !sidebarCollapsed} title={sidebarCollapsed ? "Expand Sessions" : "Collapse Sessions"}>
    <span class="sidebar-icon">
      <ClipboardText size={18} weight="light" />
    </span>
    {#if !sidebarCollapsed}
      <h2>Sessions</h2>
    {/if}
  </button>

  {#if !sidebarCollapsed}
  <div class="sidebar-content">
    <section class="section">
      <button class="section-header" onclick={() => activeCollapsed = !activeCollapsed}>
        <span class="chevron" class:collapsed={activeCollapsed}>
          {#if activeCollapsed}
            <CaretRight size={12} weight="light" />
          {:else}
            <CaretDown size={12} weight="light" />
          {/if}
        </span>
        <h3>Active</h3>
      </button>
      {#if !activeCollapsed}
        {#if $sessions.sessions.filter(s => isActiveState(s.state)).length === 0}
          <p class="empty-state">No active sessions</p>
        {:else}
          <ul class="session-list">
            {#each $sessions.sessions.filter(s => isActiveState(s.state)) as session}
              <li class="session-item" class:active={$activeSession?.id === session.id} style:--session-color={session.color || 'transparent'}>
                <div class="session-row">
                  <div
                    class="session-button"
                    role="button"
                    tabindex="0"
                    onclick={() => selectSession(session.id)}
                    onkeydown={(event) => handleSessionButtonKeydown(event, session.id)}
                  >
                    {#if editingSessionId === session.id}
                      <div class="edit-container" onclick={e => e.stopPropagation()}>
                        <input
                          type="text"
                          bind:value={editName}
                          onkeydown={handleKeydown}
                          placeholder="Session Name"
                          aria-label="Session Name"
                          autofocus
                        />
                        <div class="edit-actions">
                          <button
                            class="color-toggle"
                            onclick={() => showColorPicker = !showColorPicker}
                            title="Choose Color"
                            style:background={editColor || 'var(--bg-void)'}
                            type="button"
                          >
                          </button>
                          {#if showColorPicker}
                            <div class="color-picker">
                              {#each COLORS as color}
                                <button
                                  class="color-option"
                                  style:background={color.value}
                                  class:selected={editColor === color.value}
                                  onclick={() => { editColor = color.value; showColorPicker = false; }}
                                  title={color.name}
                                  type="button"
                                >
                                </button>
                              {/each}
                              <button
                                class="color-option clear"
                                onclick={() => { editColor = null; showColorPicker = false; }}
                                title="Clear Color"
                                type="button"
                              >×</button>
                            </div>
                          {/if}
                          <button class="save-btn" onclick={saveMetadata} title="Save" type="button">
                            <Check size={14} weight="light" />
                          </button>
                          <button class="cancel-btn-inline" onclick={cancelEdit} title="Cancel" type="button">×</button>
                        </div>
                      </div>
                    {:else}
                      <span class="session-path">
                        {session.name || session.project_path.split(/[/\\]/).pop()}
                        <button
                          class="edit-btn"
                          onclick={(e) => { e.stopPropagation(); startEdit(session); }}
                          title="Rename Session"
                          type="button"
                        >
                          <PencilSimple size={12} weight="light" />
                        </button>
                      </span>
                      <span class="session-meta">
                        {#if 'Solo' in session.session_type || ('Hive' in session.session_type && session.session_type.Hive.worker_count === 1 && session.agents.length === 1)}
                          <span class="type-tag solo">Solo</span>
                        {/if}
                        {formatTimestamp(session.created_at)}
                      </span>
                    {/if}
                  </div>
                  <button
                    class="close-session-button"
                    onclick={(e) => handleCloseSession(e, session.id)}
                    title="Close Session"
                    aria-label="Close Session"
                    disabled={closingSessionId === session.id}
                    type="button"
                  >
                    {closingSessionId === session.id ? '…' : '×'}
                  </button>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>

    <section class="section">
      <button class="section-header" onclick={() => recentCollapsed = !recentCollapsed}>
        <span class="chevron" class:collapsed={recentCollapsed}>
          {#if recentCollapsed}
            <CaretRight size={12} weight="light" />
          {:else}
            <CaretDown size={12} weight="light" />
          {/if}
        </span>
        <h3>Recent</h3>
      </button>
      {#if !recentCollapsed}
        {#if loadingPersisted}
          <p class="empty-state">Loading...</p>
        {:else if persistedSessions.length === 0}
          <p class="empty-state">No recent sessions</p>
        {:else}
          <ul class="session-list">
            {#each persistedSessions.slice(0, 5) as session}
              <li class="session-item recent">
                <div class="session-info">
                  <span class="session-path">{session.project_path.split(/[/\\]/).pop()}</span>
                  <span class="session-meta">
                    {#if session.session_type.startsWith('Solo') || (session.session_type === 'Hive (1)' && session.agent_count === 1)}
                      <span class="type-tag solo">Solo</span>
                    {/if}
                    {formatTimestamp(session.created_at)}
                  </span>
                </div>
                <button class="load-button" onclick={() => handleResumeSession(session.id)} title="Load Session">
                  <CaretRight size={14} weight="light" />
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>
  </div>
  {/if}

  <div class="sidebar-footer">
    <button class="launch-button" onclick={() => showLaunchDialog = true} title="New Session">
      <span class="icon">+</span>
      {#if !sidebarCollapsed}
        New Session
      {/if}
    </button>
  </div>
</aside>

<LaunchDialog
  show={showLaunchDialog}
  on:close={() => showLaunchDialog = false}
  on:launchHive={handleLaunchHive}
  on:launchSwarm={handleLaunchSwarm}
  on:launchFusion={handleLaunchFusion}
  on:launchSolo={handleLaunchSolo}
/>

<!-- Close confirmation dialog -->
{#if showCloseConfirm}
  <div class="confirm-overlay" onclick={dismissCloseConfirm} role="presentation">
    <div class="confirm-dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true">
      <h3>Close Session?</h3>
      <p>This will terminate all agents and mark the session as closed. This action cannot be undone.</p>
      <div class="confirm-actions">
        <button class="cancel-btn" onclick={dismissCloseConfirm} disabled={closing}>Cancel</button>
        <button class="confirm-btn" onclick={confirmCloseSession} disabled={closing}>
          {closing ? 'Closing...' : 'Close Session'}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .sidebar {
    width: 220px;
    min-width: 220px;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg-surface);
    border-right: 1px solid var(--border-structural);
    transition: width 0.2s ease, min-width 0.2s ease;
  }

  .sidebar.collapsed {
    width: 52px;
    min-width: 52px;
  }

  .sidebar-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 16px;
    border-bottom: 1px solid var(--border-structural);
    background: none;
    border-left: none;
    border-right: none;
    border-top: none;
    cursor: pointer;
    width: 100%;
    text-align: left;
  }

  .sidebar-header:hover {
    background: var(--bg-elevated);
  }

  .sidebar-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .sidebar-header h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    white-space: nowrap;
  }

  .sidebar-content {
    flex: 1;
    overflow-y: auto;
    padding: 8px 0;
  }

  .section {
    padding: 0 12px;
    margin-bottom: 16px;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 4px 0;
    margin-bottom: 8px;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
  }

  .section-header:hover h3 {
    color: var(--text-primary);
  }

  .section-header h3 {
    margin: 0;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .chevron {
    font-size: 8px;
    color: var(--text-secondary);
    transition: transform 0.2s ease;
  }

  .chevron.collapsed {
    transform: rotate(-90deg);
  }

  .empty-state {
    font-size: 12px;
    color: var(--text-secondary);
    padding: 8px 4px;
    margin: 0;
  }

  .session-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .session-item {
    margin-bottom: 4px;
    border-left: 3px solid var(--session-color);
    border-radius: var(--radius-sm);
  }

  .session-item.active .session-button {
    background: var(--bg-elevated);
    border-color: var(--accent-cyan);
  }

  .session-button {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
    background: transparent;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .edit-btn {
    opacity: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    cursor: pointer;
    color: var(--text-secondary);
    padding: 2px 4px;
    margin-left: 4px;
    transition: all 0.15s ease;
  }

  .session-path:hover .edit-btn {
    opacity: 1;
  }

  .session-path:focus-within .edit-btn,
  .edit-btn:focus-visible {
    opacity: 1;
  }

  .edit-btn:hover {
    color: var(--accent-cyan);
    background: var(--bg-void);
    border-radius: var(--radius-sm);
  }

  .edit-btn:focus-visible {
    outline: 2px solid var(--accent-cyan);
    outline-offset: 2px;
  }

  .edit-container {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .edit-container input {
    width: 100%;
    background: var(--bg-void);
    border: 1px solid var(--accent-cyan);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    padding: 4px 8px;
    font-size: 13px;
  }

  .edit-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    position: relative;
  }

  .color-toggle {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 1px solid var(--border-structural);
    cursor: pointer;
  }

  .color-picker {
    position: absolute;
    top: 24px;
    left: 0;
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    padding: 6px;
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 4px;
    z-index: 20;
    box-shadow: 0 4px 12px color-mix(in srgb, var(--bg-void) 30%, transparent);
  }

  .color-option {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1px solid color-mix(in srgb, var(--bg-void) 20%, transparent);
    cursor: pointer;
    padding: 0;
  }

  .color-option.selected {
    border: 2px solid var(--text-primary);
  }

  .color-option.clear {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    color: var(--text-secondary);
    background: var(--bg-void);
  }

  .save-btn, .cancel-btn-inline {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    cursor: pointer;
    font-size: 14px;
    padding: 2px 6px;
    border-radius: var(--radius-sm);
  }

  .save-btn {
    color: var(--status-success);
  }

  .save-btn:hover {
    background: color-mix(in srgb, var(--status-success) 10%, transparent);
  }

  .cancel-btn-inline {
    color: var(--status-error);
  }

  .cancel-btn-inline:hover {
    background: color-mix(in srgb, var(--status-error) 10%, transparent);
  }

  .session-button:hover {
    background: var(--bg-elevated);
  }

  .session-row {
    display: flex;
    align-items: stretch;
    gap: 6px;
  }

  .session-row .session-button {
    flex: 1;
    min-width: 0;
  }

  .close-session-button {
    width: 28px;
    min-width: 28px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .close-session-button:hover:not(:disabled) {
    background: var(--bg-elevated);
    border-color: var(--border-structural);
    color: var(--text-primary);
  }

  .close-session-button:disabled {
    cursor: wait;
    opacity: 0.7;
  }

  .session-path {
    font-size: 13px;
    color: var(--text-primary);
  }

  .session-meta {
    font-size: 11px;
    color: var(--text-secondary);
    margin-top: 2px;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .type-tag {
    padding: 1px 4px;
    border-radius: var(--radius-sm);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    background: var(--bg-void);
    color: var(--text-secondary);
    border: 1px solid var(--border-structural);
  }

  .type-tag.solo {
    background: color-mix(in srgb, var(--accent-cyan) 10%, transparent);
    color: var(--accent-cyan);
    border-color: color-mix(in srgb, var(--accent-cyan) 30%, transparent);
  }

  .sidebar-footer {
    padding: 12px;
    border-top: 1px solid var(--border-structural);
  }

  .launch-button {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 10px;
    border: none;
    border-radius: var(--radius-sm);
    background: var(--accent-cyan);
    color: var(--bg-void);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
    overflow: hidden;
  }

  .sidebar.collapsed .launch-button {
    padding: 10px 8px;
  }

  .launch-button:hover {
    background: var(--accent-cyan);
  }

  .launch-button .icon {
    font-size: 16px;
    font-weight: 400;
    flex-shrink: 0;
  }

  .session-item.recent {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    transition: all 0.15s ease;
  }

  .session-item.recent:hover {
    background: var(--bg-elevated);
  }

  .session-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
  }

  .load-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 4px 8px;
    border: 1px solid var(--accent-cyan);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--accent-cyan);
    cursor: pointer;
    transition: all 0.15s ease;
    flex-shrink: 0;
  }

  .load-button:hover {
    background: var(--accent-cyan);
    color: var(--bg-void);
  }

  .confirm-overlay {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--bg-void) 60%, transparent);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 10;
  }

  .confirm-dialog {
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    padding: 20px;
    width: 220px;
  }

  .confirm-dialog h3 {
    margin: 0 0 8px 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .confirm-dialog p {
    margin: 0 0 16px 0;
    font-size: 12px;
    color: var(--text-secondary);
    line-height: 1.5;
  }

  .confirm-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .cancel-btn,
  .confirm-btn {
    padding: 8px 16px;
    border: none;
    border-radius: var(--radius-sm);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .cancel-btn {
    background: var(--bg-elevated);
    color: var(--text-primary);
  }

  .cancel-btn:hover:not(:disabled) {
    background: var(--border-structural);
  }

  .confirm-btn {
    background: var(--status-error);
    color: var(--bg-void);
  }

  .confirm-btn:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .cancel-btn:disabled,
  .confirm-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
