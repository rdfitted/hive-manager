<script lang="ts">
  import { activeAgents, activeSession, sessions, serdeEnumVariantName } from '$lib/stores/sessions';
  import { coordination } from '$lib/stores/coordination';
  import Terminal from './Terminal.svelte';
  import FusionComparisonView from './fusion/FusionComparisonView.svelte';
  import ResolverPanel from './fusion/ResolverPanel.svelte';
  import { Keyboard, ChartBar, Crown, Scales, MagnifyingGlass } from 'phosphor-svelte';
  import { invoke } from '@tauri-apps/api/core';

  let fusionAgents = $derived($activeAgents.filter(a => typeof a.role === 'object' && 'Fusion' in a.role));
  let queenAgent = $derived($activeAgents.find((a) => serdeEnumVariantName(a.role) === 'Queen'));
  let judgeAgent = $derived($activeAgents.find(a => typeof a.role === 'object' && 'Judge' in a.role));
  let completedVariants = $derived($coordination.fusionState.completedVariants);
  let judgeReport = $derived($coordination.fusionState.judgeReport);
  let evaluationReady = $derived($coordination.fusionState.evaluationReady);

  let viewMode = $state<'terminals' | 'comparison'>('terminals');
  let applyingWinner = $state<string | null>(null);
  let showCleanupConfirm = $state(false);
  let error = $state<string | null>(null);

  let isResolvingOrCompleted = $derived($activeSession?.state === 'Completed' || $activeSession?.state === 'Running' && evaluationReady);
  // Note: The new SessionStatus in domain.ts includes 'resolving', but existing SessionState uses 'Running' + coordination state.

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
  <div class="panel-controls">
    <div class="view-tabs">
      <button class="tab-button" class:active={viewMode === 'terminals'} onclick={() => viewMode = 'terminals'}>
        <Keyboard size={20} weight="light" /> Terminals
      </button>
      <button class="tab-button" class:active={viewMode === 'comparison'} onclick={() => viewMode = 'comparison'}>
        <ChartBar size={20} weight="light" /> Comparison
      </button>
    </div>
  </div>

  {#if viewMode === 'terminals'}
    {#if queenAgent}
      <div class="orchestrator-section">
        <div class="orchestrator-header">
          <Crown size={24} weight="light" />
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
          <Scales size={24} weight="light" />
          <h3>Judge</h3>
          <span class="cli-badge">{judgeAgent.config?.cli || 'unknown'}</span>
        </div>
        <div class="orchestrator-terminal">
          <Terminal agentId={judgeAgent.id} isFocused={true} />
        </div>
      </div>
    {/if}
  {:else}
    <div class="comparison-container">
      {#if $activeSession}
        <FusionComparisonView sessionId={$activeSession.id} />
      {/if}
    </div>
  {/if}

  {#if isResolvingOrCompleted}
    <div class="resolver-section">
      <div class="section-header">
        <MagnifyingGlass size={24} weight="light" />
        <h3>Resolver Analysis</h3>
      </div>
      {#if $activeSession}
        <ResolverPanel sessionId={$activeSession.id} />
      {/if}
    </div>
  {/if}

  {#if evaluationReady && judgeReport}
    <div class="judge-section">
      <div class="section-header">
        <Scales size={24} weight="light" />
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

  .panel-controls {
    display: flex;
    justify-content: flex-start;
    margin-bottom: 4px;
  }

  .view-tabs {
    display: flex;
    background: var(--bg-void);
    padding: 4px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-structural);
    gap: 4px;
  }

  .tab-button {
    padding: 6px 16px;
    border-radius: var(--radius-sm);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    gap: 8px;
    transition: all 0.2s;
  }

  .tab-button:hover {
    color: var(--text-primary);
    background: color-mix(in srgb, var(--text-primary) 5%, transparent);
  }

  .tab-button.active {
    background: var(--bg-surface);
    color: var(--accent-cyan);
    box-shadow: 0 2px 4px color-mix(in srgb, var(--bg-void) 20%, transparent);
  }

  .comparison-container {
    flex: 1;
    min-height: 500px;
  }

  .resolver-section {
    background: var(--bg-surface);
    border: 1px solid var(--accent-cyan);
    border-radius: var(--radius-sm);
    overflow: hidden;
    box-shadow: 0 4px 20px color-mix(in srgb, var(--bg-void) 30%, transparent);
  }

  .orchestrator-section {
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .orchestrator-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    background: var(--bg-void);
    border-bottom: 1px solid var(--border-structural);
  }

  .orchestrator-header h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    flex: 1;
  }

  .orchestrator-terminal {
    height: 300px;
    background: var(--bg-void);
  }

  .cli-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    background: var(--accent-cyan);
    color: var(--bg-void);
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
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .variant-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 14px;
    background: var(--bg-void);
    border-bottom: 1px solid var(--border-structural);
  }

  .variant-name {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .status-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    font-weight: 500;
  }

  .status-badge.running {
    background: var(--accent-cyan);
    color: var(--bg-void);
  }

  .status-badge.completed {
    background: color-mix(in srgb, var(--status-success) 15%, transparent);
    color: var(--status-success);
  }

  .status-badge.failed {
    background: color-mix(in srgb, var(--status-error) 15%, transparent);
    color: var(--status-error);
  }

  .terminal-container {
    flex: 1;
    min-height: 250px;
    background: var(--bg-void);
  }

  .variant-actions {
    padding: 12px;
    background: var(--bg-void);
    border-top: 1px solid var(--border-structural);
    display: flex;
    justify-content: center;
  }

  .apply-button {
    padding: 8px 16px;
    background: var(--status-success);
    color: var(--bg-void);
    border: none;
    border-radius: var(--radius-sm);
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
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 16px;
    background: var(--bg-void);
    border-bottom: 1px solid var(--border-structural);
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
    font-family: var(--font-mono);
    font-size: 13px;
    color: var(--text-primary);
    line-height: 1.5;
  }

  .error-banner {
    padding: 12px;
    background: color-mix(in srgb, var(--status-error) 15%, transparent);
    color: var(--status-error);
    border: 1px solid var(--status-error);
    border-radius: var(--radius-sm);
    font-size: 13px;
  }

  .modal-overlay {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--bg-void) 70%, transparent);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    padding: 24px;
    max-width: 400px;
    text-align: center;
  }

  .modal h3 {
    margin-top: 0;
    color: var(--status-success);
  }

  .modal p {
    color: var(--text-secondary);
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
    border-radius: var(--radius-sm);
    font-weight: 600;
    cursor: pointer;
    border: none;
  }

  .modal-actions button.primary {
    background: var(--accent-cyan);
    color: var(--bg-void);
  }

  .modal-actions button.secondary {
    background: var(--border-structural);
    color: var(--text-primary);
  }
</style>
