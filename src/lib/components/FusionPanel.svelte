<script lang="ts">
  import { activeAgents, activeSession, sessions } from '$lib/stores/sessions';
  import { coordination } from '$lib/stores/coordination';
  import Terminal from './Terminal.svelte';
  import { invoke } from '@tauri-apps/api/core';

  let fusionAgents = $derived($activeAgents.filter(a => typeof a.role === 'object' && 'Fusion' in a.role));
  let queenAgent = $derived($activeAgents.find(a => a.role === 'Queen'));
  let judgeAgent = $derived($activeAgents.find(a => typeof a.role === 'object' && 'Judge' in a.role));
  let completedVariants = $derived($coordination.fusionState.completedVariants);
  let judgeReport = $derived($coordination.fusionState.judgeReport);
  let evaluationReady = $derived($coordination.fusionState.evaluationReady);

  let applyingWinner = $state<string | null>(null);
  let showCleanupConfirm = $state(false);
  let error = $state<string | null>(null);

  async function handleApplyWinner(variantName: string) {
    if (!$activeSession) return;
    applyingWinner = variantName;
    error = null;
    try {
      await sessions.applyFusionWinner($activeSession.id, variantName);
      showCleanupConfirm = true;
    } catch (e) {
      error = String(e);
    } finally {
      applyingWinner = null;
    }
  }

  async function handleCleanup() {
    if (!$activeSession) return;
    try {
      await sessions.stopSession($activeSession.id);
      showCleanupConfirm = false;
    } catch (e) {
      error = String(e);
    }
  }

  function getVariantStatus(variantName: string): 'Running' | 'Completed' | 'Failed' {
    if (completedVariants.includes(variantName)) return 'Completed';
    const agent = fusionAgents.find(a => typeof a.role === 'object' && 'Fusion' in a.role && a.role.Fusion.variant === variantName);
    if (agent?.status === 'Completed') return 'Completed';
    if (agent?.status && typeof agent.status === 'object' && 'Error' in agent.status) return 'Failed';
    return 'Running';
  }
</script>

<div class="fusion-panel">
  {#if queenAgent}
    <div class="orchestrator-section">
      <div class="orchestrator-header">
        <span class="icon">♕</span>
        <h3>Fusion Queen</h3>
        <span class="cli-badge">{queenAgent.config?.cli || 'unknown'}</span>
      </div>
      <div class="orchestrator-terminal">
        <Terminal agentId={queenAgent.id} isFocused={true} />
      </div>
    </div>
  {/if}

  <div class="variants-grid" class:has-report={evaluationReady}>
    {#each fusionAgents as agent (agent.id)}
      {@const variantName = typeof agent.role === 'object' && 'Fusion' in agent.role ? agent.role.Fusion.variant : ''}
      {@const status = getVariantStatus(variantName)}
      <div class="variant-card">
        <div class="variant-header">
          <span class="variant-name">{variantName}</span>
          <span class="status-badge" class:running={status === 'Running'} class:completed={status === 'Completed'} class:failed={status === 'Failed'}>
            {status}
          </span>
        </div>
        <div class="terminal-container">
          <Terminal agentId={agent.id} isFocused={true} />
        </div>
        {#if evaluationReady}
          <div class="variant-actions">
            <button 
              class="apply-button" 
              onclick={() => handleApplyWinner(variantName)}
              disabled={applyingWinner !== null}
            >
              {applyingWinner === variantName ? 'Applying...' : 'Apply as Winner'}
            </button>
          </div>
        {/if}
      </div>
    {/each}
  </div>

  {#if judgeAgent}
    <div class="orchestrator-section">
      <div class="orchestrator-header">
        <span class="icon">⚖️</span>
        <h3>Judge</h3>
        <span class="cli-badge">{judgeAgent.config?.cli || 'unknown'}</span>
      </div>
      <div class="orchestrator-terminal">
        <Terminal agentId={judgeAgent.id} isFocused={true} />
      </div>
    </div>
  {/if}

  {#if evaluationReady && judgeReport}
    <div class="judge-section">
      <div class="section-header">
        <span class="icon">⚖️</span>
        <h3>Judge Evaluation Report</h3>
      </div>
      <div class="report-content">
        <pre>{judgeReport}</pre>
      </div>
    </div>
  {/if}

  {#if error}
    <div class="error-banner">{error}</div>
  {/if}

  {#if showCleanupConfirm}
    <div class="modal-overlay">
      <div class="modal">
        <h3>Winner Applied</h3>
        <p>The changes from the selected variant have been applied to the project. Would you like to close this session now?</p>
        <div class="modal-actions">
          <button class="secondary" onclick={() => showCleanupConfirm = false}>Keep Open</button>
          <button class="primary" onclick={handleCleanup}>Close Session</button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .fusion-panel {
    display: flex;
    flex-direction: column;
    gap: 20px;
    height: 100%;
    padding: 16px;
    overflow-y: auto;
  }

  .orchestrator-section {
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 8px;
    overflow: hidden;
  }

  .orchestrator-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    background: var(--color-bg);
    border-bottom: 1px solid var(--color-border);
  }

  .orchestrator-header h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    flex: 1;
  }

  .orchestrator-terminal {
    height: 300px;
    background: #000;
  }

  .cli-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 10px;
    background: var(--color-primary-muted);
    color: var(--color-accent);
    font-weight: 500;
  }

  .variants-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(400px, 1fr));
    gap: 16px;
    min-height: 400px;
  }

  .variants-grid.has-report {
    min-height: 300px;
  }

  .variant-card {
    display: flex;
    flex-direction: column;
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 8px;
    overflow: hidden;
  }

  .variant-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 14px;
    background: var(--color-bg);
    border-bottom: 1px solid var(--color-border);
  }

  .variant-name {
    font-size: 13px;
    font-weight: 600;
    color: var(--color-text);
  }

  .status-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 10px;
    font-weight: 500;
  }

  .status-badge.running {
    background: var(--color-primary-muted);
    color: var(--color-accent);
  }

  .status-badge.completed {
    background: rgba(158, 206, 106, 0.15);
    color: var(--color-success);
  }

  .status-badge.failed {
    background: rgba(247, 118, 142, 0.15);
    color: var(--color-error);
  }

  .terminal-container {
    flex: 1;
    min-height: 250px;
    background: #000;
  }

  .variant-actions {
    padding: 12px;
    background: var(--color-bg);
    border-top: 1px solid var(--color-border);
    display: flex;
    justify-content: center;
  }

  .apply-button {
    padding: 8px 16px;
    background: var(--color-success);
    color: var(--color-bg);
    border: none;
    border-radius: 4px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: filter 0.15s;
  }

  .apply-button:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .apply-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .judge-section {
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 8px;
    overflow: hidden;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 16px;
    background: var(--color-bg);
    border-bottom: 1px solid var(--color-border);
  }

  .section-header h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }

  .report-content {
    padding: 16px;
    max-height: 500px;
    overflow-y: auto;
  }

  .report-content pre {
    margin: 0;
    white-space: pre-wrap;
    font-family: 'Fira Code', monospace;
    font-size: 13px;
    color: var(--color-text);
    line-height: 1.5;
  }

  .error-banner {
    padding: 12px;
    background: rgba(247, 118, 142, 0.15);
    color: var(--color-error);
    border: 1px solid var(--color-error);
    border-radius: 4px;
    font-size: 13px;
  }

  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 8px;
    padding: 24px;
    max-width: 400px;
    text-align: center;
  }

  .modal h3 {
    margin-top: 0;
    color: var(--color-success);
  }

  .modal p {
    color: var(--color-text-muted);
    line-height: 1.5;
    margin-bottom: 24px;
  }

  .modal-actions {
    display: flex;
    justify-content: center;
    gap: 12px;
  }

  .modal-actions button {
    padding: 8px 16px;
    border-radius: 4px;
    font-weight: 600;
    cursor: pointer;
    border: none;
  }

  .modal-actions button.primary {
    background: var(--color-accent);
    color: var(--color-bg);
  }

  .modal-actions button.secondary {
    background: var(--color-border);
    color: var(--color-text);
  }
</style>
