<script lang="ts">
  import { sessions, activeSession, type Session, type AgentInfo, type HiveLaunchConfig, type SwarmLaunchConfig } from '$lib/stores/sessions';
  import LaunchDialog from './LaunchDialog.svelte';

  interface Props {
    onLaunch: (projectPath: string, workerCount: number, command: string, prompt?: string) => Promise<void>;
    onLaunchHiveV2?: (config: HiveLaunchConfig) => Promise<void>;
    onLaunchSwarm?: (config: SwarmLaunchConfig) => Promise<void>;
  }

  let { onLaunch, onLaunchHiveV2, onLaunchSwarm }: Props = $props();

  let showLaunchDialog = $state(false);
  let launching = $state(false);
  let sidebarCollapsed = $state(true);
  let activeCollapsed = $state(false);
  let recentCollapsed = $state(true);

  function getStatusIcon(status: AgentInfo['status']): string {
    if (status === 'Running') return '█';
    if (typeof status === 'object' && 'WaitingForInput' in status) return '⏳';
    if (status === 'Completed') return '✓';
    if (status === 'Starting') return '○';
    if (typeof status === 'object' && 'Error' in status) return '✗';
    return '?';
  }

  function getStatusColor(status: AgentInfo['status']): string {
    if (status === 'Running') return 'var(--color-running)';
    if (typeof status === 'object' && 'WaitingForInput' in status) return 'var(--color-warning)';
    if (status === 'Completed') return 'var(--color-success)';
    if (status === 'Starting') return 'var(--color-text-muted)';
    if (typeof status === 'object' && 'Error' in status) return 'var(--color-error)';
    return 'var(--color-text)';
  }

  function getRoleName(role: AgentInfo['role']): string {
    if (role === 'Queen') return 'Queen';
    if (typeof role === 'object') {
      if ('Planner' in role) return `Planner ${role.Planner.index}`;
      if ('Worker' in role) return `Worker ${role.Worker.index}`;
      if ('Fusion' in role) return `Fusion: ${role.Fusion.variant}`;
    }
    return 'Agent';
  }

  function getSessionTypeName(session: Session): string {
    if ('Hive' in session.session_type) return 'Hive';
    if ('Swarm' in session.session_type) return 'Swarm';
    if ('Fusion' in session.session_type) return 'Fusion';
    return 'Session';
  }

  function selectSession(sessionId: string) {
    sessions.setActiveSession(sessionId);
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
        {#if $sessions.sessions.filter(s => s.state === 'Running' || s.state === 'Starting').length === 0}
          <p class="empty-state">No active sessions</p>
        {:else}
          <ul class="session-list">
            {#each $sessions.sessions.filter(s => s.state === 'Running' || s.state === 'Starting') as session}
              <li class="session-item" class:active={$activeSession?.id === session.id}>
                <button class="session-button" onclick={() => selectSession(session.id)}>
                  <span class="session-type">{getSessionTypeName(session)}</span>
                  <span class="session-path">{session.project_path.split(/[/\\]/).pop()}</span>
                </button>
                <ul class="agent-list">
                  {#each session.agents.slice(0, 4) as agent}
                    <li class="agent-item">
                      <span class="agent-status" style="color: {getStatusColor(agent.status)}">
                        {getStatusIcon(agent.status)}
                      </span>
                      <span class="agent-name">{agent.config?.label || getRoleName(agent.role)}</span>
                    </li>
                  {/each}
                  {#if session.agents.length > 4}
                    <li class="agent-item more">
                      <span class="agent-name">+{session.agents.length - 4} more</span>
                    </li>
                  {/if}
                </ul>
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
        {#if $sessions.sessions.filter(s => s.state === 'Completed').length === 0}
          <p class="empty-state">No recent sessions</p>
        {:else}
          <ul class="session-list">
            {#each $sessions.sessions.filter(s => s.state === 'Completed').slice(0, 5) as session}
              <li class="session-item completed">
                <button class="session-button" onclick={() => selectSession(session.id)}>
                  <span class="session-type">{getSessionTypeName(session)}</span>
                  <span class="session-path">{session.project_path.split(/[/\\]/).pop()}</span>
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

  .session-item.completed {
    opacity: 0.6;
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

  .session-type {
    font-size: 11px;
    font-weight: 600;
    color: var(--color-accent);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .session-path {
    font-size: 13px;
    color: var(--color-text);
    margin-top: 2px;
  }

  .agent-list {
    list-style: none;
    margin: 4px 0 0 12px;
    padding: 0;
  }

  .agent-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 0;
    font-size: 12px;
  }

  .agent-item.more {
    color: var(--color-text-muted);
    font-style: italic;
  }

  .agent-status {
    font-size: 10px;
  }

  .agent-name {
    color: var(--color-text-muted);
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
</style>
