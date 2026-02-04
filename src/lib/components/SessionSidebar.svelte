<script lang="ts">
  import { sessions, activeSession, type Session, type AgentInfo } from '$lib/stores/sessions';
  import { settings } from '$lib/stores/settings';
  import { open } from '@tauri-apps/plugin-dialog';

  interface Props {
    onLaunch: (projectPath: string, workerCount: number, command: string, prompt?: string) => Promise<void>;
  }

  let { onLaunch }: Props = $props();

  let showLaunchDialog = $state(false);
  let projectPath = $state('');
  let workerCount = $state(2);
  let command = $state('claude');  // Default to claude, but user can change
  let prompt = $state('');
  let launching = $state(false);
  let launchError = $state('');

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

  async function handleLaunch() {
    if (!projectPath.trim() || !command.trim()) return;
    launching = true;
    launchError = '';
    try {
      await onLaunch(projectPath, workerCount, command, prompt || undefined);
      showLaunchDialog = false;
      projectPath = '';
      prompt = '';
    } catch (err) {
      launchError = String(err);
    } finally {
      launching = false;
    }
  }

  function selectSession(sessionId: string) {
    sessions.setActiveSession(sessionId);
  }

  async function browseForFolder() {
    const selected = await open({
      directory: true,
      multiple: false,
      title: 'Select Project Folder'
    });
    if (selected && typeof selected === 'string') {
      projectPath = selected;
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
                {#each session.agents as agent}
                  <li class="agent-item">
                    <span class="agent-status" style="color: {getStatusColor(agent.status)}">
                      {getStatusIcon(agent.status)}
                    </span>
                    <span class="agent-name">{getRoleName(agent.role)}</span>
                  </li>
                {/each}
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
    <button class="launch-button" onclick={() => { showLaunchDialog = true; launchError = ''; }}>
      <span class="icon">+</span> New Session
    </button>
  </div>
</aside>

{#if showLaunchDialog}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="dialog-overlay" onclick={() => showLaunchDialog = false} role="presentation">
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" tabindex="-1">
      <h2>Launch New Session</h2>
      <form onsubmit={(e) => { e.preventDefault(); handleLaunch(); }}>
        <div class="form-group">
          <label for="projectPath">Project Path</label>
          <div class="path-picker">
            <input
              id="projectPath"
              type="text"
              bind:value={projectPath}
              placeholder="Select a project folder..."
              readonly
              required
            />
            <button type="button" class="browse-button" onclick={browseForFolder}>
              Browse
            </button>
          </div>
        </div>
        <div class="form-group">
          <label for="command">Command</label>
          <input
            id="command"
            type="text"
            bind:value={command}
            placeholder="claude, cmd.exe, powershell, or any .bat"
          />
        </div>
        <div class="form-group">
          <label for="workerCount">Workers</label>
          <div class="worker-buttons">
            {#each [2, 3, 4] as count}
              <button
                type="button"
                class="worker-button"
                class:selected={workerCount === count}
                onclick={() => workerCount = count}
              >
                {count}
              </button>
            {/each}
          </div>
        </div>
        <div class="form-group">
          <label for="prompt">Initial Prompt (optional)</label>
          <textarea
            id="prompt"
            bind:value={prompt}
            placeholder="Enter a task for the hive..."
            rows="3"
          ></textarea>
        </div>
        {#if launchError}
          <div class="error-message">{launchError}</div>
        {/if}
        <div class="dialog-actions">
          <button type="button" class="cancel-button" onclick={() => showLaunchDialog = false} disabled={launching}>
            Cancel
          </button>
          <button type="submit" class="submit-button" disabled={launching || !projectPath.trim()}>
            {launching ? 'Launching...' : 'Launch'}
          </button>
        </div>
      </form>
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

  /* Dialog styles */
  .dialog-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }

  .dialog {
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 8px;
    padding: 24px;
    width: 420px;
    max-width: 90vw;
  }

  .dialog h2 {
    margin: 0 0 20px 0;
    font-size: 18px;
    color: var(--color-text);
  }

  .form-group {
    margin-bottom: 16px;
  }

  .form-group label {
    display: block;
    margin-bottom: 6px;
    font-size: 13px;
    font-weight: 500;
    color: var(--color-text);
  }

  .form-group input,
  .form-group textarea {
    width: 100%;
    padding: 10px 12px;
    border: 1px solid var(--color-border);
    border-radius: 6px;
    background: var(--color-bg);
    color: var(--color-text);
    font-size: 14px;
    font-family: inherit;
  }

  .form-group input:focus,
  .form-group textarea:focus {
    outline: none;
    border-color: var(--color-accent);
  }

  .path-picker {
    display: flex;
    gap: 8px;
  }

  .path-picker input {
    flex: 1;
    cursor: pointer;
  }

  .path-picker input:read-only {
    background: var(--color-surface);
  }

  .browse-button {
    padding: 10px 16px;
    border: 1px solid var(--color-border);
    border-radius: 6px;
    background: var(--color-surface-hover);
    color: var(--color-text);
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.15s ease;
  }

  .browse-button:hover {
    background: var(--color-border);
    border-color: var(--color-accent);
  }

  .worker-buttons {
    display: flex;
    gap: 8px;
  }

  .worker-button {
    flex: 1;
    padding: 10px;
    border: 1px solid var(--color-border);
    border-radius: 6px;
    background: var(--color-bg);
    color: var(--color-text);
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .worker-button:hover {
    border-color: var(--color-accent);
  }

  .worker-button.selected {
    background: var(--color-accent);
    border-color: var(--color-accent);
    color: var(--color-bg);
  }

  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 12px;
    margin-top: 24px;
  }

  .cancel-button,
  .submit-button {
    padding: 10px 20px;
    border: none;
    border-radius: 6px;
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .cancel-button {
    background: var(--color-surface-hover);
    color: var(--color-text);
  }

  .cancel-button:hover {
    background: var(--color-border);
  }

  .submit-button {
    background: var(--color-accent);
    color: var(--color-bg);
  }

  .submit-button:hover:not(:disabled) {
    background: var(--color-accent-bright);
  }

  .submit-button:disabled,
  .cancel-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .error-message {
    padding: 12px;
    margin-bottom: 16px;
    background: rgba(247, 118, 142, 0.15);
    border: 1px solid var(--color-error);
    border-radius: 6px;
    color: var(--color-error);
    font-size: 13px;
    word-break: break-word;
  }
</style>
