<script lang="ts">
  import { activeSession, activeAgents, sessions, type AgentInfo, type Session } from '$lib/stores/sessions';
  import { ui } from '$lib/stores/ui';
  import AgentTree from './AgentTree.svelte';

  let collapsed = $state(true);
  let agentsCollapsed = $state(false);
  let alertsCollapsed = $state(false);
  let infoCollapsed = $state(true);
  let showCloseConfirm = $state<string | null>(null);
  let closing = $state(false);

  function handleAlertClick(agentId: string) {
    ui.setFocusedAgent(agentId);
  }

  function getSessionStateClass(state: Session['state']): string {
    if (typeof state === 'string') return state.toLowerCase();
    if (typeof state === 'object' && 'Failed' in state) return 'failed';
    return 'unknown';
  }

  function getSessionStateText(state: Session['state']): string {
    if (typeof state === 'string') return state;
    if (typeof state === 'object' && 'Failed' in state) return `Failed: ${state.Failed}`;
    return 'Unknown';
  }

  function getRoleName(role: AgentInfo['role']): string {
    if (role === 'Queen') return 'Queen';
    if (typeof role === 'object') {
      if ('Planner' in role) return `Planner ${role.Planner.index}`;
      if ('Worker' in role) return `Worker ${role.Worker.index}`;
      if ('Fusion' in role) return role.Fusion.variant;
    }
    return 'Agent';
  }

  function getAgentLabel(agent: AgentInfo): string {
    return agent.config?.label || getRoleName(agent.role);
  }

  function isSessionActive(state: Session['state']): boolean {
    if (state === 'Completed' || state === 'Closed') return false;
    if (typeof state === 'object' && state !== null && 'Failed' in state) return false;
    return true;
  }

  async function handleCloseSession() {
    const sessionId = showCloseConfirm;
    if (!sessionId) return;
    closing = true;
    try {
      await sessions.closeSession(sessionId);
      showCloseConfirm = null;
    } catch (err) {
      console.error('Failed to close session:', err);
    } finally {
      closing = false;
    }
  }

  function dismissCloseConfirm() {
    if (!closing) {
      showCloseConfirm = null;
    }
  }
</script>

<aside class="status-panel" class:collapsed>
  <button class="panel-header" onclick={() => collapsed = !collapsed} title={collapsed ? "Expand Status" : "Collapse Status"}>
    <span class="panel-icon">📊</span>
    {#if !collapsed}
      <h2>Status</h2>
    {/if}
  </button>

  {#if !collapsed}
    {#if !$activeSession}
      <div class="empty-state">
        <p>No session selected</p>
        <p class="hint">Launch a new session to get started</p>
      </div>
    {:else}
      <div class="panel-content">
        <section class="section">
          <button class="section-header" onclick={() => agentsCollapsed = !agentsCollapsed}>
            <span class="chevron" class:collapsed={agentsCollapsed}>▼</span>
            <h3>Agents ({$activeAgents.length})</h3>
          </button>
          {#if !agentsCollapsed}
            <div class="section-content">
              <AgentTree agents={$activeAgents} selectedId={null} />
            </div>
          {/if}
        </section>

        <section class="section">
          <button class="section-header" onclick={() => alertsCollapsed = !alertsCollapsed}>
            <span class="chevron" class:collapsed={alertsCollapsed}>▼</span>
            <h3>Alerts</h3>
          </button>
          {#if !alertsCollapsed}
            <div class="alerts">
              {#each $activeAgents.filter(a => typeof a.status === 'object' && 'WaitingForInput' in a.status) as agent}
                {@const lastLine = typeof agent.status === 'object' && 'WaitingForInput' in agent.status ? agent.status.WaitingForInput : ''}
                <button class="alert warning clickable" onclick={() => handleAlertClick(agent.id)}>
                  <div class="alert-header">
                    <span class="alert-icon">⚠</span>
                    <span class="alert-title">{getAgentLabel(agent)} needs input</span>
                  </div>
                  {#if lastLine}
                    <div class="alert-body">
                      <p class="last-line">{lastLine}</p>
                    </div>
                  {/if}
                </button>
              {:else}
                <p class="no-alerts">No alerts</p>
              {/each}
            </div>
          {/if}
        </section>

        <section class="section">
          <button class="section-header" onclick={() => infoCollapsed = !infoCollapsed}>
            <span class="chevron" class:collapsed={infoCollapsed}>▼</span>
            <h3>Session Info</h3>
          </button>
          {#if !infoCollapsed}
            <div class="info-grid">
              <div class="info-item">
                <span class="info-label">Type</span>
                <span class="info-value">
                  {'Hive' in $activeSession.session_type ? 'Hive' :
                   'Swarm' in $activeSession.session_type ? 'Swarm' : 'Fusion'}
                </span>
              </div>
              <div class="info-item">
                <span class="info-label">Agents</span>
                <span class="info-value">{$activeAgents.length}</span>
              </div>
              <div class="info-item">
                <span class="info-label">State</span>
                <span class="info-value state-{getSessionStateClass($activeSession.state)}">
                  {getSessionStateText($activeSession.state)}
                </span>
              </div>
            </div>
          {/if}
        </section>

        {#if isSessionActive($activeSession.state)}
          <section class="section actions-section">
            <button
              class="close-button"
              onclick={() => showCloseConfirm = $activeSession?.id ?? null}
              title="Close this session (kills all agents and marks as closed)"
            >
              Close Session
            </button>
          </section>
        {/if}
      </div>

      <!-- Close confirmation dialog -->
      {#if showCloseConfirm}
        <div class="confirm-overlay" onclick={dismissCloseConfirm} role="presentation">
          <div class="confirm-dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true">
            <h3>Close Session?</h3>
            <p>This will terminate all agents and mark the session as closed. This action cannot be undone.</p>
            <div class="confirm-actions">
              <button class="cancel-btn" onclick={dismissCloseConfirm} disabled={closing}>Cancel</button>
              <button class="confirm-btn" onclick={handleCloseSession} disabled={closing}>
                {closing ? 'Closing...' : 'Close Session'}
              </button>
            </div>
          </div>
        </div>
      {/if}
    {/if}
  {/if}
</aside>

<style>
  .status-panel {
    width: 240px;
    min-width: 240px;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--color-surface);
    border-left: 1px solid var(--color-border);
    transition: width 0.2s ease, min-width 0.2s ease;
  }

  .status-panel.collapsed {
    width: 52px;
    min-width: 52px;
  }

  .panel-header {
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

  .panel-header:hover {
    background: var(--color-surface-hover);
  }

  .panel-icon {
    font-size: 18px;
    flex-shrink: 0;
  }

  .panel-header h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--color-text);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    white-space: nowrap;
  }

  .panel-content {
    flex: 1;
    overflow-y: auto;
    padding: 8px 0;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    padding: 20px;
    text-align: center;
  }

  .empty-state p {
    margin: 0;
    color: var(--color-text-muted);
    font-size: 13px;
  }

  .empty-state .hint {
    margin-top: 8px;
    font-size: 12px;
    opacity: 0.7;
  }

  .section {
    padding: 8px 16px;
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

  .section-content {
    max-height: 300px;
    overflow-y: auto;
  }

  .alerts {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .alert {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px 12px;
    border-radius: 4px;
    font-size: 12px;
  }

  .alert.warning {
    background: rgba(224, 175, 104, 0.15);
    color: var(--color-warning);
  }

  .alert.clickable {
    cursor: pointer;
    border: 1px solid transparent;
    width: 100%;
    text-align: left;
    transition: all 0.2s;
  }

  .alert.clickable:hover {
    background: rgba(224, 175, 104, 0.25);
    border-color: var(--color-warning);
  }

  .alert-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .alert-title {
    font-weight: 600;
  }

  .alert-body {
    padding-left: 22px;
    margin-top: 2px;
  }

  .last-line {
    margin: 0;
    font-size: 11px;
    font-family: 'Cascadia Code', Consolas, monospace;
    opacity: 0.8;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--color-text);
  }

  .alert-icon {
    font-size: 14px;
  }

  .no-alerts {
    margin: 0;
    font-size: 12px;
    color: var(--color-text-muted);
    text-align: center;
    padding: 8px;
  }

  .info-grid {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .info-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 0;
  }

  .info-label {
    font-size: 12px;
    color: var(--color-text-muted);
  }

  .info-value {
    font-size: 13px;
    font-weight: 500;
    color: var(--color-text);
  }

  .info-value.state-running {
    color: var(--color-running);
  }

  .info-value.state-completed {
    color: var(--color-success);
  }

  .info-value.state-failed {
    color: var(--color-error);
  }

  .info-value.state-closed {
    color: var(--color-text-muted);
  }

  .actions-section {
    margin-top: auto;
    padding-top: 12px;
    border-top: 1px solid var(--color-border);
  }

  .close-button {
    width: 100%;
    padding: 10px 12px;
    border: 1px solid var(--color-error);
    border-radius: 6px;
    background: transparent;
    color: var(--color-error);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .close-button:hover {
    background: var(--color-error);
    color: var(--color-bg);
  }

  .confirm-overlay {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 10;
  }

  .confirm-dialog {
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 8px;
    padding: 20px;
    width: 220px;
    max-width: 90%;
  }

  .confirm-dialog h3 {
    margin: 0 0 12px 0;
    font-size: 15px;
    color: var(--color-text);
  }

  .confirm-dialog p {
    margin: 0 0 16px 0;
    font-size: 12px;
    color: var(--color-text-muted);
    line-height: 1.4;
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
    border-radius: 4px;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .cancel-btn {
    background: var(--color-surface-hover);
    color: var(--color-text);
  }

  .cancel-btn:hover:not(:disabled) {
    background: var(--color-border);
  }

  .confirm-btn {
    background: var(--color-error);
    color: var(--color-bg);
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
