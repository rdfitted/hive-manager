<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { activeAgents, activeSession, sessions, serdeEnumVariantName } from '$lib/stores/sessions';
  import { coordination } from '$lib/stores/coordination';
  import Terminal from './Terminal.svelte';
  import { Keyboard, ChartBar, Crown, Scales, MagnifyingGlass, GitBranch, GitPullRequest, Warning } from 'phosphor-svelte';
  import { apiUrl } from '$lib/config';

  // Interfaces for Debate API
  interface DebateDebaterStatus {
    index: number;
    name: string;
    stance: string | null;
    branch: string;
    worktree_path: string;
    status: string;
  }

  interface DebateStatusResponse {
    session_id: string;
    state: string;
    debaters: DebateDebaterStatus[];
  }

  interface DebateEvaluationResponse {
    session_id: string;
    state: string;
    report_path: string;
    report: string | null;
  }

  let debateStatus = $state<DebateStatusResponse | null>(null);
  let debateEvaluation = $state<DebateEvaluationResponse | null>(null);
  let error = $state<string | null>(null);
  let viewMode = $state<'terminals' | 'comparison' | 'verdict'>('terminals');
  let pollInterval = $state<any>(null);
  let endingSession = $state(false);

  // Derivations
  let queenAgent = $derived($activeAgents.find((a) => serdeEnumVariantName(a.role) === 'Queen'));
  let judgeAgent = $derived($activeAgents.find(a => typeof a.role === 'object' && 'Judge' in a.role));
  
  // Debaters from status response
  let debaters = $derived(debateStatus?.debaters ?? []);

  // Filter agents matching debaters (role has Fusion/Variant or matches debater names)
  let debaterAgents = $derived($activeAgents.filter(a => 
    (typeof a.role === 'object' && 'Fusion' in a.role) || 
    debaters.some(d => d.name === a.config.label || a.id.includes(d.name.toLowerCase()))
  ));

  // State evaluation ready
  let evaluationReady = $derived(!!debateEvaluation?.report);
  let judgeReport = $derived(debateEvaluation?.report ?? null);

  // Extract PR URL from verdict report or coordination logs
  let prUrl = $derived(extractPrUrl(judgeReport, $coordination.log));

  function extractPrUrl(reportText: string | null, logs: any[]): string | null {
    const prRegex = /https:\/\/github\.com\/[^/]+\/[^/]+\/pull\/\d+/;
    if (reportText) {
      const match = reportText.match(prRegex);
      if (match) return match[0];
    }
    for (const log of logs) {
      if (log.content) {
        const match = log.content.match(prRegex);
        if (match) return match[0];
      }
    }
    return null;
  }

  async function fetchDebateData() {
    const sessionId = $activeSession?.id;
    if (!sessionId) return;

    const isCurrentSession = () => $activeSession?.id === sessionId;

    try {
      const statusRes = await fetch(apiUrl(`/api/sessions/${sessionId}/debate/status`));
      if (statusRes.ok) {
        const status = await statusRes.json();
        if (!isCurrentSession()) return;
        debateStatus = status;
      }

      if (!isCurrentSession()) return;
      
      const evalRes = await fetch(apiUrl(`/api/sessions/${sessionId}/debate/evaluation`));
      if (evalRes.ok) {
        const evaluation = await evalRes.json();
        if (!isCurrentSession()) return;
        debateEvaluation = evaluation;
        
        // Auto-switch to verdict tab when it becomes ready
        if (debateEvaluation?.report && viewMode === 'terminals') {
          viewMode = 'verdict';
        }
      }
    } catch (e) {
      console.error("Error fetching debate data:", e);
    }
  }

  onMount(() => {
    fetchDebateData();
    pollInterval = setInterval(fetchDebateData, 3000);
  });

  onDestroy(() => {
    if (pollInterval) clearInterval(pollInterval);
  });

  async function handleCloseSession() {
    if (!$activeSession) return;
    endingSession = true;
    error = null;
    try {
      await sessions.stopSession($activeSession.id);
    } catch (e) {
      error = String(e);
    } finally {
      endingSession = false;
    }
  }

  function getDebaterStatusClass(status: string): string {
    const s = status.toLowerCase();
    if (s.includes('running') || s.includes('active')) return 'running';
    if (s.includes('completed') || s.includes('success')) return 'completed';
    if (s.includes('failed') || s.includes('error')) return 'failed';
    return 'queued';
  }
</script>

<div class="debate-panel">
  <!-- Debate Header Bar -->
  <div class="debate-header-bar">
    <div class="info-group">
      <div class="state-tag" class:eval-ready={evaluationReady}>
        {#if evaluationReady}
          Verdict Ready
        {:else if $activeSession?.state}
          {$activeSession.state}
        {:else}
          Running
        {/if}
      </div>
      <div class="session-name">{$activeSession?.name || 'Debate Session'}</div>
    </div>
    
    <div class="panel-controls">
      <div class="view-tabs">
        <button class="tab-button" class:active={viewMode === 'terminals'} onclick={() => viewMode = 'terminals'}>
          <Keyboard size={18} weight="light" /> Terminals
        </button>
        <button class="tab-button" class:active={viewMode === 'comparison'} onclick={() => viewMode = 'comparison'}>
          <ChartBar size={18} weight="light" /> Debaters
        </button>
        <button class="tab-button" class:active={viewMode === 'verdict'} onclick={() => viewMode = 'verdict'}>
          <Scales size={18} weight="light" /> Judge Verdict
        </button>
      </div>
    </div>
  </div>

  {#if error}
    <div class="error-banner">
      <Warning size={16} />
      <span>{error}</span>
    </div>
  {/if}

  {#if viewMode === 'terminals'}
    <!-- TERMINALS VIEW -->
    <div class="terminals-layout">
      {#if queenAgent}
        <div class="orchestrator-section">
          <div class="section-header">
            <Crown size={20} weight="light" class="icon-queen" />
            <h3>Orchestrator Queen</h3>
            <span class="cli-badge">{queenAgent.config?.cli || 'unknown'}</span>
          </div>
          <div class="orchestrator-terminal">
            <Terminal agentId={queenAgent.id} isFocused={true} />
          </div>
        </div>
      {/if}

      <div class="debaters-terminals-grid">
        {#each debaterAgents as agent (agent.id)}
          <div class="variant-card">
            <div class="variant-header">
              <span class="variant-name">{agent.config?.label || 'Debater'}</span>
              <span class="cli-badge">{agent.config?.cli || 'unknown'}</span>
            </div>
            <div class="terminal-container">
              <Terminal agentId={agent.id} isFocused={true} />
            </div>
          </div>
        {/each}
      </div>

      {#if judgeAgent}
        <div class="orchestrator-section">
          <div class="section-header">
            <Scales size={20} weight="light" class="icon-judge" />
            <h3>Debate Judge</h3>
            <span class="cli-badge">{judgeAgent.config?.cli || 'unknown'}</span>
          </div>
          <div class="orchestrator-terminal">
            <Terminal agentId={judgeAgent.id} isFocused={true} />
          </div>
        </div>
      {/if}
    </div>

  {:else if viewMode === 'comparison'}
    <!-- DEBATERS COMPARISON VIEW -->
    <div class="comparison-layout">
      <div class="debaters-grid">
        {#each debaters as debater (debater.index)}
          <div class="debater-status-card" class:completed={debater.status === 'Completed'}>
            <div class="debater-card-header">
              <div class="status-indicator">
                <span class="status-dot {getDebaterStatusClass(debater.status)}"></span>
                <span class="debater-name">{debater.name}</span>
              </div>
              <span class="status-text">{debater.status}</span>
            </div>

            <div class="debater-card-body">
              {#if debater.stance}
                <div class="data-row">
                  <span class="data-label">Stance:</span>
                  <span class="data-value stance-badge">{debater.stance}</span>
                </div>
              {/if}

              <div class="data-row">
                <span class="data-label">Branch:</span>
                <span class="data-value code-font">
                  <GitBranch size={12} /> {debater.branch}
                </span>
              </div>

              <div class="data-row">
                <span class="data-label">Worktree:</span>
                <span class="data-value code-font filepath" title={debater.worktree_path}>
                  {debater.worktree_path}
                </span>
              </div>
            </div>
          </div>
        {/each}
      </div>
    </div>

  {:else}
    <!-- JUDGE VERDICT VIEW -->
    <div class="verdict-layout">
      {#if prUrl}
        <div class="pr-banner">
          <div class="pr-icon-wrapper">
            <GitPullRequest size={24} />
          </div>
          <div class="pr-info">
            <h4>Captured Pull Request</h4>
            <p>The judge has successfully proposed and opened a GitHub PR containing the winning adjustments.</p>
          </div>
          <a href={prUrl} target="_blank" rel="noopener noreferrer" class="pr-link-button">
            View Pull Request
          </a>
        </div>
      {/if}

      <div class="verdict-card">
        <div class="verdict-header">
          <Scales size={22} weight="light" />
          <h3>Structured Verdict</h3>
          {#if debateEvaluation?.report_path}
            <span class="report-path-badge" title={debateEvaluation.report_path}>{debateEvaluation.report_path}</span>
          {/if}
        </div>
        
        <div class="verdict-body">
          {#if judgeReport}
            <pre class="verdict-report">{judgeReport}</pre>
          {:else}
            <div class="verdict-empty">
              <div class="spinner"></div>
              <span>The judge is currently reviewing arguments and compiling the final verdict...</span>
            </div>
          {/if}
        </div>
      </div>

      {#if evaluationReady}
        <div class="verdict-actions-bar">
          <button class="close-session-btn" onclick={handleCloseSession} disabled={endingSession}>
            {endingSession ? 'Closing...' : 'Close Session'}
          </button>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .debate-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    padding: 16px;
    background: var(--bg-void);
    gap: 16px;
    overflow-y: auto;
  }

  .debate-header-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-bottom: 1px solid var(--border-structural);
    padding-bottom: 12px;
  }

  .info-group {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .state-tag {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    background: var(--accent-cyan);
    color: var(--bg-void);
    padding: 3px 8px;
    border-radius: var(--radius-sm);
    letter-spacing: 0.05em;
  }

  .state-tag.eval-ready {
    background: var(--status-success);
  }

  .session-name {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .view-tabs {
    display: flex;
    background: color-mix(in srgb, var(--text-primary) 3%, var(--bg-surface));
    padding: 4px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-structural);
    gap: 4px;
  }

  .tab-button {
    padding: 6px 14px;
    border-radius: var(--radius-sm);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    gap: 6px;
    transition: all 0.2s;
  }

  .tab-button:hover {
    color: var(--text-primary);
    background: color-mix(in srgb, var(--text-primary) 4%, transparent);
  }

  .tab-button.active {
    background: var(--bg-surface);
    color: var(--accent-cyan);
    box-shadow: 0 1px 3px color-mix(in srgb, var(--bg-void) 20%, transparent);
  }

  .error-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px;
    background: color-mix(in srgb, var(--status-error) 12%, transparent);
    color: var(--status-error);
    border: 1px solid color-mix(in srgb, var(--status-error) 25%, transparent);
    border-radius: var(--radius-sm);
    font-size: 13px;
  }

  /* Terminals View Styles */
  .terminals-layout {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .orchestrator-section {
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    background: color-mix(in srgb, var(--text-primary) 2%, var(--bg-surface));
    border-bottom: 1px solid var(--border-structural);
  }

  .section-header h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    flex: 1;
  }

  :global(.icon-queen) {
    color: var(--status-warning);
  }

  :global(.icon-judge) {
    color: var(--accent-cyan);
  }

  .orchestrator-terminal {
    height: 250px;
    background: var(--bg-void);
  }

  .debaters-terminals-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(400px, 1fr));
    gap: 16px;
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
    background: color-mix(in srgb, var(--text-primary) 2%, var(--bg-surface));
    border-bottom: 1px solid var(--border-structural);
  }

  .variant-name {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .cli-badge {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: var(--radius-sm);
    background: var(--border-structural);
    color: var(--text-secondary);
    font-family: var(--font-mono);
  }

  .terminal-container {
    height: 350px;
    background: var(--bg-void);
  }

  /* Comparison View Styles */
  .comparison-layout {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .debaters-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 16px;
  }

  .debater-status-card {
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
    transition: all 0.2s ease;
  }

  .debater-status-card.completed {
    border-color: color-mix(in srgb, var(--status-success) 20%, transparent);
    background: color-mix(in srgb, var(--status-success) 2%, var(--bg-surface));
  }

  .debater-card-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px;
    background: color-mix(in srgb, var(--text-primary) 2%, var(--bg-surface));
    border-bottom: 1px solid var(--border-structural);
  }

  .status-indicator {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    display: inline-block;
  }

  .status-dot.queued { background: var(--text-disabled); }
  .status-dot.running { background: var(--accent-cyan); }
  .status-dot.completed { background: var(--status-success); }
  .status-dot.failed { background: var(--status-error); }

  .debater-name {
    font-weight: 600;
    font-size: 14px;
    color: var(--text-primary);
  }

  .status-text {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
  }

  .debater-card-body {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .data-row {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .data-label {
    font-size: 11px;
    color: var(--text-disabled);
    text-transform: uppercase;
    font-weight: 700;
  }

  .data-value {
    font-size: 13px;
    color: var(--text-primary);
  }

  .stance-badge {
    align-self: flex-start;
    background: color-mix(in srgb, var(--accent-cyan) 12%, transparent);
    color: var(--accent-cyan);
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    font-weight: 600;
  }

  .code-font {
    font-family: var(--font-mono);
    font-size: 12px;
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-secondary);
  }

  .filepath {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Verdict View Styles */
  .verdict-layout {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .pr-banner {
    display: flex;
    align-items: center;
    background: color-mix(in srgb, var(--status-success) 10%, var(--bg-surface));
    border: 1px solid color-mix(in srgb, var(--status-success) 20%, transparent);
    border-radius: var(--radius-sm);
    padding: 16px;
    gap: 16px;
  }

  .pr-icon-wrapper {
    color: var(--status-success);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .pr-info {
    flex: 1;
  }

  .pr-info h4 {
    margin: 0 0 4px 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--status-success);
  }

  .pr-info p {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .pr-link-button {
    padding: 8px 16px;
    background: var(--status-success);
    color: var(--bg-void);
    border-radius: var(--radius-sm);
    font-size: 13px;
    font-weight: 600;
    text-decoration: none;
    transition: filter 0.2s;
  }

  .pr-link-button:hover {
    filter: brightness(1.1);
  }

  .verdict-card {
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .verdict-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 16px;
    background: color-mix(in srgb, var(--text-primary) 2%, var(--bg-surface));
    border-bottom: 1px solid var(--border-structural);
  }

  .verdict-header h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }

  .report-path-badge {
    font-size: 10px;
    font-family: var(--font-mono);
    color: var(--text-disabled);
    background: color-mix(in srgb, var(--bg-void) 40%, transparent);
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    margin-left: auto;
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .verdict-body {
    padding: 20px;
    min-height: 250px;
  }

  .verdict-report {
    margin: 0;
    white-space: pre-wrap;
    font-family: var(--font-mono);
    font-size: 13px;
    line-height: 1.6;
    color: var(--text-primary);
  }

  .verdict-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 16px;
    height: 200px;
    color: var(--text-disabled);
    font-size: 13px;
    text-align: center;
  }

  .spinner {
    width: 28px;
    height: 28px;
    border: 2px solid color-mix(in srgb, var(--text-primary) 10%, transparent);
    border-top-color: var(--accent-cyan);
    border-radius: 50%;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .verdict-actions-bar {
    display: flex;
    justify-content: flex-end;
    margin-top: 8px;
  }

  .close-session-btn {
    padding: 10px 20px;
    background: var(--border-structural);
    color: var(--text-primary);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.2s;
  }

  .close-session-btn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--text-primary) 8%, var(--bg-surface));
    border-color: color-mix(in srgb, var(--text-primary) 12%, var(--bg-surface));
  }

  .close-session-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
