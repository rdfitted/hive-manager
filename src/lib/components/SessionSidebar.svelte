<script lang="ts">
  import { CaretDown, CaretLeft, CaretRight, Check, House, Kanban, PencilSimple } from 'phosphor-svelte';
  import { page } from '$app/stores';
  import { sessions, activeSession, activeAgents, serdeEnumVariantName, type Session, type ResumeReport, type HiveLaunchConfig, type ResearchLaunchConfig, type SwarmLaunchConfig, type FusionLaunchConfig, type SoloLaunchConfig, type DebateLaunchConfig } from '$lib/stores/sessions';
  import { layout, RAIL_WIDTH } from '$lib/stores/layout';
  import { ui } from '$lib/stores/ui';
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import LaunchDialog from './LaunchDialog.svelte';
  import AgentTree from './AgentTree.svelte';
  import QueenControls from './QueenControls.svelte';
  import ResizeHandle from './ResizeHandle.svelte';
  import ResumeConfirmModal from './ResumeConfirmModal.svelte';

  let closingSessionId = $state<string | null>(null);
  let showCloseConfirm = $state<string | null>(null);
  let closing = $state(false);
  let resizing = $state(false);

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

  interface Props {
    onLaunch: (projectPath: string, workerCount: number, command: string, prompt?: string) => Promise<void>;
    onLaunchHiveV2?: (config: HiveLaunchConfig) => Promise<void>;
    onLaunchResearch?: (config: ResearchLaunchConfig) => Promise<void>;
    onLaunchSwarm?: (config: SwarmLaunchConfig) => Promise<void>;
    onLaunchFusion?: (config: FusionLaunchConfig) => Promise<void>;
    onLaunchSolo?: (config: SoloLaunchConfig) => Promise<void>;
    onLaunchDebate?: (config: DebateLaunchConfig) => Promise<void>;
    onOpenAddWorker?: () => void;
  }

  interface SessionSummary {
    id: string;
    session_type: string;
    project_path: string;
    created_at: string;
    last_activity_at?: string;
    agent_count: number;
    state: string;
  }

  let { onLaunch, onLaunchHiveV2, onLaunchResearch, onLaunchSwarm, onLaunchFusion, onLaunchSolo, onLaunchDebate, onOpenAddWorker }: Props = $props();

  let showLaunchDialog = $state(false);
  let launching = $state(false);
  let persistedSessions = $state<SessionSummary[]>([]);
  let loadingPersisted = $state(false);
  let currentDirectory = $state<string | null>(null);
  let resumeModalOpen = $state(false);
  let resumeTargetSessionId = $state<string | null>(null);
  let resumeTargetName = $state<string | null>(null);
  let resumeReport = $state<ResumeReport | null>(null);
  let resumeLoading = $state(false);
  let resuming = $state(false);
  let resumeError = $state<string | null>(null);

  let collapsed = $derived($layout.leftCollapsed);
  let sidebarWidth = $derived(collapsed ? RAIL_WIDTH : $layout.leftWidth);

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

  function handleAgentSelect(e: CustomEvent<string>) {
    ui.setFocusedAgent(e.detail);
    ui.setSelectedAgent(e.detail);
  }

  function sessionDisplayName(session: SessionSummary): string {
    return session.project_path.split(/[/\\]/).pop() || session.id.slice(0, 8);
  }

  async function handleResumeSession(session: SessionSummary) {
    const targetSessionId = session.id;
    resumeTargetSessionId = targetSessionId;
    resumeTargetName = sessionDisplayName(session);
    resumeReport = null;
    resumeError = null;
    resumeModalOpen = true;
    resumeLoading = true;

    try {
      resumeReport = await sessions.getResumeReport(targetSessionId);
    } catch (err) {
      resumeError = String(err);
      console.error('Failed to prepare resume:', err);
    } finally {
      resumeLoading = false;
    }
  }

  function resetResumeModal() {
    resumeModalOpen = false;
    resumeTargetSessionId = null;
    resumeTargetName = null;
    resumeReport = null;
    resumeError = null;
  }

  function dismissResumeModal() {
    if (resuming) return;
    resetResumeModal();
  }

  async function confirmResumeSession(e: CustomEvent<{ skipCompletedWriteSteps: boolean }>) {
    const targetSessionId = resumeTargetSessionId;
    if (!targetSessionId) return;

    resuming = true;
    resumeError = null;
    try {
      await sessions.resumeSession(targetSessionId, e.detail);
      // Remove from persisted list after successful resume
      persistedSessions = persistedSessions.filter(s => s.id !== targetSessionId);
      resetResumeModal();
    } catch (err) {
      resumeError = String(err);
      console.error('Failed to resume session:', err);
    } finally {
      resuming = false;
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

  async function handleLaunchResearch(e: CustomEvent<ResearchLaunchConfig>) {
    launching = true;
    try {
      if (onLaunchResearch) {
        await onLaunchResearch(e.detail);
      } else {
        await sessions.launchResearch(e.detail);
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

  async function handleLaunchDebate(e: CustomEvent<DebateLaunchConfig>) {
    launching = true;
    try {
      if (onLaunchDebate) {
        await onLaunchDebate(e.detail);
      } else {
        await sessions.launchDebate(e.detail);
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

  /** Focus the rename input when it appears, without the a11y-flagged autofocus attribute. */
  function focusOnMount(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
</script>

<aside
  class="sidebar"
  class:collapsed
  class:resizing
  style:width={`${sidebarWidth}px`}
  style:min-width={`${sidebarWidth}px`}
>
  <div class="sidebar-header" class:collapsed>
    <nav class="view-toggle" class:collapsed aria-label="View switcher">
      {#each [
        { path: '/', icon: House, label: 'Sessions' },
        { path: '/dashboard', icon: Kanban, label: 'Dashboard' }
      ] as { path, icon: Icon, label } (path)}
        <a
          href={path}
          class="view-link"
          class:active={$page.url.pathname === path}
          aria-label={label}
          aria-current={$page.url.pathname === path ? 'page' : undefined}
          title={label}
        >
          <Icon size={18} weight="light" />
        </a>
      {/each}
    </nav>
    <button
      type="button"
      class="collapse-chevron"
      onclick={() => layout.toggleLeft()}
      title={collapsed ? "Expand sidebar (Ctrl+B)" : "Collapse sidebar (Ctrl+B)"}
      aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
    >
      {#if collapsed}
        <CaretRight size={14} weight="light" />
      {:else}
        <CaretLeft size={14} weight="light" />
      {/if}
    </button>
  </div>

  {#if !collapsed}
  <div class="sidebar-content">
    <section class="section">
      <button class="section-header" onclick={() => layout.toggleSection('sessionsCollapsed')}>
        <span class="chevron" class:collapsed={$layout.sessionsCollapsed}>
          {#if $layout.sessionsCollapsed}
            <CaretRight size={12} weight="light" />
          {:else}
            <CaretDown size={12} weight="light" />
          {/if}
        </span>
        <h3>Active</h3>
      </button>
      {#if !$layout.sessionsCollapsed}
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
                      <div
                        class="edit-container"
                        role="presentation"
                        onclick={e => e.stopPropagation()}
                        onkeydown={e => e.stopPropagation()}
                      >
                        <input
                          type="text"
                          bind:value={editName}
                          onkeydown={handleKeydown}
                          placeholder="Session Name"
                          aria-label="Session Name"
                          use:focusOnMount
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
                          <button class="save-btn" onclick={saveMetadata} title="Save" aria-label="Save session metadata" type="button">
                            <Check size={14} weight="light" />
                          </button>
                          <button class="cancel-btn-inline" onclick={cancelEdit} title="Cancel" aria-label="Cancel edit" type="button">×</button>
                        </div>
                      </div>
                    {:else}
                      <span class="session-path">
                        {session.name || session.project_path.split(/[/\\]/).pop()}
                        <button
                          class="edit-btn"
                          onclick={(e) => { e.stopPropagation(); startEdit(session); }}
                          title="Rename Session"
                          aria-label="Rename session"
                          type="button"
                        >
                          <PencilSimple size={12} weight="light" />
                        </button>
                      </span>
                      <span class="session-meta">
                        {#if 'Solo' in session.session_type || ('Hive' in session.session_type && session.session_type.Hive.worker_count === 1 && session.agents.length === 1)}
                          <span class="type-tag solo">Solo</span>
                        {/if}
                        {formatTimestamp(session.last_activity_at ?? session.created_at)}
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
      <button class="section-header" onclick={() => layout.toggleSection('recentCollapsed')}>
        <span class="chevron" class:collapsed={$layout.recentCollapsed}>
          {#if $layout.recentCollapsed}
            <CaretRight size={12} weight="light" />
          {:else}
            <CaretDown size={12} weight="light" />
          {/if}
        </span>
        <h3>Recent</h3>
      </button>
      {#if !$layout.recentCollapsed}
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
                    {formatTimestamp(session.last_activity_at ?? session.created_at)}
                  </span>
                </div>
                <button class="load-button" onclick={() => handleResumeSession(session)} title="Load Session" aria-label="Load session" type="button">
                  <CaretRight size={14} weight="light" />
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>

    {#if $activeSession}
      <section class="section">
        <button class="section-header" onclick={() => layout.toggleSection('agentsCollapsed')}>
          <span class="chevron" class:collapsed={$layout.agentsCollapsed}>
            {#if $layout.agentsCollapsed}
              <CaretRight size={12} weight="light" />
            {:else}
              <CaretDown size={12} weight="light" />
            {/if}
          </span>
          <h3>Agents ({$activeAgents.length})</h3>
        </button>
        {#if !$layout.agentsCollapsed}
          <AgentTree
            agents={$activeAgents}
            selectedId={$ui.focusedAgentId}
            on:select={handleAgentSelect}
          />
          <div class="queen-controls-section">
            <QueenControls on:openAddWorker={() => onOpenAddWorker?.()} />
          </div>
        {/if}
      </section>
    {/if}
  </div>
  {/if}

  <div class="sidebar-footer">
    <button class="launch-button" onclick={() => showLaunchDialog = true} title="New Session">
      <span class="icon">+</span>
      {#if !collapsed}
        New Session
      {/if}
    </button>
  </div>

  {#if !collapsed}
    <ResizeHandle
      label="Resize sidebar"
      onResize={(clientX) => layout.setLeftWidth(clientX)}
      onDragChange={(d) => resizing = d}
    />
  {/if}
</aside>

<LaunchDialog
  show={showLaunchDialog}
  on:close={() => showLaunchDialog = false}
  on:launchHive={handleLaunchHive}
  on:launchResearch={handleLaunchResearch}
  on:launchSwarm={handleLaunchSwarm}
  on:launchFusion={handleLaunchFusion}
  on:launchSolo={handleLaunchSolo}
  on:launchDebate={handleLaunchDebate}
/>

<ResumeConfirmModal
  open={resumeModalOpen}
  sessionName={resumeTargetName}
  report={resumeReport}
  loading={resumeLoading}
  confirming={resuming}
  error={resumeError}
  on:confirm={confirmResumeSession}
  on:cancel={dismissResumeModal}
/>

<!-- Close confirmation dialog -->
{#if showCloseConfirm}
  <div
    class="confirm-overlay"
    onclick={dismissCloseConfirm}
    onkeydown={(event) => event.key === 'Escape' && dismissCloseConfirm()}
    role="presentation"
  >
    <div
      class="confirm-dialog"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Escape') dismissCloseConfirm(); }}
      role="dialog"
      aria-modal="true"
      tabindex="-1"
    >
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
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg-surface);
    border-right: 1px solid var(--border-structural);
    transition: width 0.2s ease, min-width 0.2s ease;
  }

  .sidebar.resizing {
    transition: none;
  }

  .sidebar :global(.resize-handle) {
    right: -3px;
  }

  .sidebar-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px;
    border-bottom: 1px solid var(--border-structural);
    width: 100%;
  }

  .sidebar-header.collapsed {
    flex-direction: column;
    gap: 6px;
  }

  .view-toggle {
    display: flex;
    flex-direction: row;
    gap: 4px;
    flex: 1;
    min-width: 0;
  }

  .view-toggle.collapsed {
    flex-direction: column;
    flex: none;
  }

  .view-link {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: var(--radius-sm, 2px);
    color: var(--text-muted);
    text-decoration: none;
    border: 1px solid transparent;
    transition: color 0.15s ease, background 0.15s ease, border-color 0.15s ease;
  }

  .view-link:hover {
    color: var(--accent-cyan);
  }

  .view-link.active {
    color: var(--accent-cyan);
    background: var(--bg-elevated);
    border-color: var(--border-structural);
  }

  .collapse-chevron {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    border-radius: var(--radius-sm, 2px);
    flex-shrink: 0;
  }

  .collapse-chevron:hover {
    background: var(--bg-elevated);
    color: var(--accent-cyan);
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

  .queen-controls-section {
    margin-top: 8px;
    border-top: 1px solid var(--border-structural);
    padding-top: 8px;
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
