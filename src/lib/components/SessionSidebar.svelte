<script lang="ts">
  import { sessions, activeSession, type Session, type HiveLaunchConfig, type SwarmLaunchConfig, type FusionLaunchConfig, type SoloLaunchConfig } from '$lib/stores/sessions';
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';

  let closingSessionId = $state<string | null>(null);

  async function handleCloseSession(e: Event, sessionId: string) {
    e.stopPropagation();
    closingSessionId = sessionId;
    try {
      await sessions.closeSession(sessionId);
    } catch (err) {
      console.error('Failed to close session:', err);
    } finally {
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
    return state === 'Running' || state === 'Starting' || state === 'Planning' || state === 'PlanReady';
  }

  function selectSession(sessionId: string) {
    sessions.setActiveSession(sessionId);
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
</script>

<aside class="sidebar" class:collapsed={sidebarCollapsed}>
  <button class="sidebar-header" onclick={() => sidebarCollapsed = !sidebarCollapsed} title={sidebarCollapsed ? "Expand Sessions" : "Collapse Sessions"}>
    <span class="sidebar-icon">📋</span>
    {#if !sidebarCollapsed}
      <h2>Sessions</h2>
    {/if}
  </button>

  {#if !sidebarCollapsed}
  <div class="sidebar-content">
    <section class="section">
      <button class="section-header" onclick={() => activeCollapsed = !activeCollapsed}>
        <span class="chevron" class:collapsed={activeCollapsed}>▼</span>
        <h3>Active</h3>
      </button>
      {#if !activeCollapsed}
        {#if $sessions.sessions.filter(s => isActiveState(s.state)).length === 0}
          <p class="empty-state">No active sessions</p>
        {:else}
          <ul class="session-list">
            {#each $sessions.sessions.filter(s => isActiveState(s.state)) as session}
              <li class="session-item" class:active={$activeSession?.id === session.id}>
                <div class="session-row">
                  <button class="session-button" onclick={() => selectSession(session.id)}>
                    <span class="session-path">{session.project_path.split(/[/\\]/).pop()}</span>
                    <span class="session-meta">
                      {#if 'Solo' in session.session_type || ('Hive' in session.session_type && session.session_type.Hive.worker_count === 1 && session.agents.length === 1)}
                        <span class="type-tag solo">Solo</span>
                      {/if}
                      {formatTimestamp(session.created_at)}
                    </span>
                  </button>
                  <button
                    class="close-session-button"
                    onclick={(e) => handleCloseSession(e, session.id)}
                    title="Close Session"
                    aria-label="Close Session"
                    disabled={closingSessionId === session.id}
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
        <span class="chevron" class:collapsed={recentCollapsed}>▼</span>
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
                  ▶
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

<style>
  .sidebar {
    width: 220px;
    min-width: 220px;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--color-surface);
    border-right: 1px solid var(--color-border);
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
    border-bottom: 1px solid var(--color-border);
    background: none;
    border-left: none;
    border-right: none;
    border-top: none;
    cursor: pointer;
    width: 100%;
    text-align: left;
  }

  .sidebar-header:hover {
    background: var(--color-surface-hover);
  }

  .sidebar-icon {
    font-size: 18px;
    flex-shrink: 0;
  }

  .sidebar-header h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--color-text);
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
    color: var(--color-text);
  }

  .section-header h3 {
    margin: 0;
    font-size: 11px;
    font-weight: 600;
    color: var(--color-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .chevron {
    font-size: 8px;
    color: var(--color-text-muted);
    transition: transform 0.2s ease;
  }

  .chevron.collapsed {
    transform: rotate(-90deg);
  }

  .empty-state {
    font-size: 12px;
    color: var(--color-text-muted);
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
  }

  .session-item.active .session-button {
    background: var(--color-accent-dim);
    border-color: var(--color-accent);
  }

  .session-button {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-radius: 4px;
    background: transparent;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .session-button:hover {
    background: var(--color-surface-hover);
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
    border-radius: 4px;
    background: transparent;
    color: var(--color-text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .close-session-button:hover:not(:disabled) {
    background: var(--color-surface-hover);
    border-color: var(--color-border);
    color: var(--color-text);
  }

  .close-session-button:disabled {
    cursor: wait;
    opacity: 0.7;
  }

  .session-path {
    font-size: 13px;
    color: var(--color-text);
  }

  .session-meta {
    font-size: 11px;
    color: var(--color-text-muted);
    margin-top: 2px;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .type-tag {
    padding: 1px 4px;
    border-radius: 3px;
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    background: var(--color-bg-secondary);
    color: var(--color-text-muted);
    border: 1px solid var(--color-border);
  }

  .type-tag.solo {
    background: rgba(122, 162, 247, 0.1);
    color: var(--color-accent);
    border-color: rgba(122, 162, 247, 0.3);
  }

  .sidebar-footer {
    padding: 12px;
    border-top: 1px solid var(--color-border);
  }

  .launch-button {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 10px;
    border: none;
    border-radius: 6px;
    background: var(--color-accent);
    color: var(--color-bg);
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
    background: var(--color-accent-bright);
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
    border-radius: 4px;
    transition: all 0.15s ease;
  }

  .session-item.recent:hover {
    background: var(--color-surface-hover);
  }

  .session-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
  }

  .load-button {
    padding: 4px 8px;
    border: 1px solid var(--color-accent);
    border-radius: 4px;
    background: transparent;
    color: var(--color-accent);
    font-size: 12px;
    cursor: pointer;
    transition: all 0.15s ease;
    flex-shrink: 0;
  }

  .load-button:hover {
    background: var(--color-accent);
    color: var(--color-bg);
  }
</style>
