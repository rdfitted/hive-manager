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

  function getStatusIcon(status: AgentInfo['status']): string {
    if (status === 'Running') return '█';
    if (status === 'WaitingForInput') return '⏳';
    if (status === 'Completed') return '✓';
    if (status === 'Starting') return '○';
    if (typeof status === 'object' && 'Error' in status) return '✗';
    return '?';
  }

  function getStatusColor(status: AgentInfo['status']): string {
    if (status === 'Running') return 'var(--color-running)';
    if (status === 'WaitingForInput') return 'var(--color-warning)';
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

<aside class="sidebar">
  <div class="sidebar-header">
    <h2>Sessions</h2>
  </div>

  <div class="sidebar-content">
    <section class="section">
      <h3>Active</h3>
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
    </section>

    <section class="section">
      <h3>Recent</h3>
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
    </section>
  </div>

  <div class="sidebar-footer">
    <button class="launch-button" onclick={() => showLaunchDialog = true}>
      <span class="icon">+</span> New Session
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
  }

  .sidebar-header {
    padding: 16px;
    border-bottom: 1px solid var(--color-border);
  }

  .sidebar-header h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--color-text);
    text-transform: uppercase;
    letter-spacing: 0.5px;
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

  .section h3 {
    margin: 0 0 8px 0;
    padding: 4px 0;
    font-size: 11px;
    font-weight: 600;
    color: var(--color-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
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
  }

  .launch-button:hover {
    background: var(--color-accent-bright);
  }

  .launch-button .icon {
    font-size: 16px;
    font-weight: 400;
  }
</style>
