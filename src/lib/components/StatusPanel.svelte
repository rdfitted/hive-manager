<script lang="ts">
  import { activeSession, activeAgents, type AgentInfo, type Session } from '$lib/stores/sessions';

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

  function getStatusText(status: AgentInfo['status']): string {
    if (status === 'Running') return 'Running';
    if (status === 'WaitingForInput') return 'Waiting for input';
    if (status === 'Completed') return 'Completed';
    if (status === 'Starting') return 'Starting';
    if (typeof status === 'object' && 'Error' in status) return `Error: ${status.Error}`;
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

  function getRoleIcon(role: AgentInfo['role']): string {
    if (role === 'Queen') return '♕';
    if (typeof role === 'object') {
      if ('Planner' in role) return '◆';
      if ('Worker' in role) return '●';
      if ('Fusion' in role) return '◎';
    }
    return '○';
  }
</script>

<aside class="status-panel">
  <div class="panel-header">
    <h2>Status</h2>
  </div>

  {#if !$activeSession}
    <div class="empty-state">
      <p>No session selected</p>
      <p class="hint">Launch a new session to get started</p>
    </div>
  {:else}
    <div class="panel-content">
      <section class="section">
        <h3>Hierarchy</h3>
        <div class="hierarchy">
          {#each $activeAgents as agent}
            <div class="hierarchy-item">
              <span class="role-icon">{getRoleIcon(agent.role)}</span>
              <span class="role-name">{getRoleName(agent.role)}</span>
              <span class="status-badge" style="color: {getStatusColor(agent.status)}">
                {getStatusIcon(agent.status)}
              </span>
            </div>
          {/each}
        </div>
      </section>

      <section class="section">
        <h3>Alerts</h3>
        <div class="alerts">
          {#each $activeAgents.filter(a => a.status === 'WaitingForInput') as agent}
            <div class="alert warning">
              <span class="alert-icon">⚠</span>
              <span class="alert-text">{getRoleName(agent.role)} needs input</span>
            </div>
          {:else}
            <p class="no-alerts">No alerts</p>
          {/each}
        </div>
      </section>

      <section class="section">
        <h3>Session Info</h3>
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
      </section>
    </div>
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
  }

  .panel-header {
    padding: 16px;
    border-bottom: 1px solid var(--color-border);
  }

  .panel-header h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--color-text);
    text-transform: uppercase;
    letter-spacing: 0.5px;
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
    padding: 12px 16px;
  }

  .section h3 {
    margin: 0 0 12px 0;
    font-size: 11px;
    font-weight: 600;
    color: var(--color-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .hierarchy {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .hierarchy-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    background: var(--color-bg);
    border-radius: 4px;
  }

  .role-icon {
    font-size: 12px;
    opacity: 0.7;
  }

  .role-name {
    flex: 1;
    font-size: 13px;
    color: var(--color-text);
  }

  .status-badge {
    font-size: 10px;
  }

  .alerts {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .alert {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-radius: 4px;
    font-size: 12px;
  }

  .alert.warning {
    background: rgba(224, 175, 104, 0.15);
    color: var(--color-warning);
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
</style>
