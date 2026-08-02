<script lang="ts">
  import { onMount } from 'svelte';
  import { CaretDown, CaretRight, Warning } from 'phosphor-svelte';
  import { activeSession, activeAgents, sessions, serdeEnumVariantName, type AgentInfo, type Session } from '$lib/stores/sessions';
  import { ui } from '$lib/stores/ui';
  import { apiUrl } from '$lib/config';
  import { cliOptions } from '$lib/config/clis';
  import {
    cliHealthLabel,
    cliHealthMessage,
    cliHealthTone,
    fetchCliHealth,
    type CliHealthMap,
  } from './AgentConfigEditor.svelte';
  import QaFeedbackPanel from './QaFeedbackPanel.svelte';
  import { invoke } from '@tauri-apps/api/core';

  let alertsCollapsed = $state(false);
  let infoCollapsed = $state(true);
  let showCloseConfirm = $state<string | null>(null);
  let closing = $state(false);
  let showForceFailConfirm = $state(false);
  let forceFailing = $state(false);
  // #176: Force Pass / Skip QA are destructive one-click overrides. Server-side
  // `confirm: true` protects nothing when the UI supplies it automatically, so
  // the real guard for an operator misclick has to live here.
  let showForcePassConfirm = $state(false);
  let showSkipQaConfirm = $state(false);
  // Operator-action failures were previously console-only: a 400 produced zero
  // on-screen change. There is no toast system in this repo, so surface inline.
  let opError = $state<string | null>(null);
  let cliHealthCollapsed = $state(false);
  let cliHealth = $state<CliHealthMap>({});
  let cliHealthLoading = $state(false);
  let cliHealthError = $state<string | null>(null);

  // Milestone tracking
  let completedMilestones = $state(0);
  let totalMilestones = $state(0);

  type SessionPlan = {
    tasks?: Array<{ status?: string }>;
  };

  async function loadCliHealth(force = false) {
    cliHealthLoading = true;
    cliHealthError = null;
    try {
      cliHealth = await fetchCliHealth(force);
    } catch (err) {
      cliHealthError = err instanceof Error ? err.message : String(err);
    } finally {
      cliHealthLoading = false;
    }
  }

  onMount(() => {
    void loadCliHealth();
  });

  $effect(() => {
    if ($activeSession?.id) {
      loadMilestoneCounts($activeSession.id);
    }
  });

  async function loadMilestoneCounts(sessionId: string) {
    try {
      const plan = await invoke<SessionPlan>('get_session_plan', { sessionId });
      if (plan && plan.tasks) {
        totalMilestones = plan.tasks.length;
        completedMilestones = plan.tasks.filter((task) => task.status === 'completed').length;
      }
    } catch (e) {
      // Plan might not exist yet
    }
  }

  function handleAlertClick(agentId: string) {
    ui.setFocusedAgent(agentId);
  }

  function getSessionStateClass(state: Session['state']): string {
    if (typeof state === 'object' && state !== null) {
      if ('Failed' in state) return 'failed';
      if ('QaFailed' in state) return 'warning';
    }
    const v = serdeEnumVariantName(state);
    if (v === 'SpawningEvaluator') return 'starting';
    if (v === 'QaInProgress') return 'running';
    if (v === 'PrinceRemediation') return 'running';
    if (v === 'QaPassed') return 'completed';
    if (v === 'QaInconclusive') return 'warning';
    if (v === 'QaMaxRetriesExceeded') return 'failed';
    if (v) return v.toLowerCase();
    return 'unknown';
  }

  function getSessionStateText(state: Session['state']): string {
    if (typeof state === 'object' && state !== null) {
      if ('Failed' in state) return `Failed: ${state.Failed}`;
      if ('QaFailed' in state) return `QA Failed (iteration ${state.QaFailed.iteration})`;
    }
    const v = serdeEnumVariantName(state);
    if (v === 'SpawningEvaluator') return 'Spawning Evaluator';
    if (v === 'QaInProgress') return 'QA In Progress';
    if (v === 'PrinceRemediation') return 'Prince Remediation';
    if (v === 'QaPassed') return 'QA Passed';
    if (v === 'QaInconclusive') return 'QA Inconclusive — operator action needed';
    if (v === 'QaMaxRetriesExceeded') return 'QA Max Retries Exceeded';
    return v ?? 'Unknown';
  }

  function getRoleName(role: AgentInfo['role']): string {
    if (typeof role === 'object' && role !== null) {
      if ('Judge' in role) return 'Judge';
      if ('Planner' in role) return `Planner ${role.Planner.index}`;
      if ('Worker' in role) return `Worker ${role.Worker.index}`;
      if ('QaWorker' in role) return `QA Worker ${role.QaWorker.index}`;
      if ('Fusion' in role) return role.Fusion.variant;
    }
    const k = serdeEnumVariantName(role);
    if (k === 'Queen') return 'Queen';
    if (k === 'Evaluator') return 'Evaluator';
    if (k === 'MasterPlanner') return 'Master Planner';
    return 'Agent';
  }

  function getAgentLabel(agent: AgentInfo): string {
    return agent.config?.label || getRoleName(agent.role);
  }

  function isSessionActive(state: Session['state']): boolean {
    if (typeof state === 'object' && state !== null && 'Failed' in state) return false;
    const v = serdeEnumVariantName(state);
    if (v === 'Completed' || v === 'Closed') return false;
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

  function dismissForceFailConfirm() {
    if (!forceFailing) {
      showForceFailConfirm = false;
    }
  }

  function handleCloseDialogKeydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === 'Escape') dismissCloseConfirm();
  }

  function handleForceFailDialogKeydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === 'Escape') dismissForceFailConfirm();
  }

  // Operator controls — use HTTP API (Tauri commands not yet registered)
  // #176: force-pass / force-fail now require an explicit `{"confirm": true}`
  // body. A bodyless POST is refused with a 400.
  async function postSessionAction(
    path: string,
    errorMessage: string,
    body?: unknown
  ): Promise<boolean> {
    const sessionId = $activeSession?.id;
    if (!sessionId) return false;
    try {
      const res = await fetch(apiUrl(`/api/sessions/${sessionId}${path}`), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body ?? {})
      });
      if (!res.ok) {
        const raw = await res.text();
        let detail = raw;
        try {
          detail = (JSON.parse(raw) as { error?: string }).error ?? raw;
        } catch {
          // non-JSON body — surface it verbatim
        }
        console.error(errorMessage, raw);
        opError = `${errorMessage} ${detail}`;
        return false;
      }
      opError = null;
      return true;
    } catch (err) {
      console.error(errorMessage, err);
      opError = `${errorMessage} ${err instanceof Error ? err.message : String(err)}`;
      return false;
    }
  }

  async function handleSkipQa() {
    const ok = await postSessionAction('/qa/force-pass', 'Failed to skip QA:', {
      confirm: true,
      rationale: 'Skip QA from the Status panel'
    });
    if (ok) showSkipQaConfirm = false;
  }

  async function handleForcePass() {
    const ok = await postSessionAction('/qa/force-pass', 'Failed to force pass milestone:', {
      confirm: true,
      rationale: 'Force pass from the Status panel'
    });
    if (ok) showForcePassConfirm = false;
  }

  async function handleForceFail() {
    forceFailing = true;
    try {
      const ok = await postSessionAction('/qa/force-fail', 'Failed to force fail milestone:', {
        confirm: true,
        rationale: 'Force fail from the Status panel'
      });
      if (ok) {
        showForceFailConfirm = false;
      }
    } finally {
      forceFailing = false;
    }
  }

  // Unchanged — still gates the QA Feedback panel. Deliberately NOT widened:
  // that section should only appear once QA has actually started.
  function isQaPhase(state: Session['state']): boolean {
    const v = serdeEnumVariantName(state);
    return (
      v === 'SpawningEvaluator' ||
      v === 'QaInProgress' ||
      v === 'PrinceRemediation' ||
      v === 'QaInconclusive' ||
      v === 'QaPassed' ||
      v === 'QaMaxRetriesExceeded' ||
      (typeof state === 'object' && state !== null && 'QaFailed' in state)
    );
  }

  // #175: evaluator-backed sessions now stay in Running until the milestone
  // handoff, but still cannot complete without reaching QaPassed — so the
  // operator needs the override controls before QA has ever started. Mirrors the
  // backend gate, which widens to Running/SpawningEvaluator only when the session
  // actually has an Evaluator or QA worker.
  function canOverrideQa(state: Session['state'], agents: Session['agents']): boolean {
    if (isQaPhase(state)) return true;
    const v = serdeEnumVariantName(state);
    if (v !== 'Running' && v !== 'SpawningEvaluator') return false;
    return (agents ?? []).some((a) => {
      const role = serdeEnumVariantName(a.role);
      return role === 'Evaluator' || role === 'QaWorker';
    });
  }
</script>

<div class="status-content">
      <div class="panel-content lattice-scroll-chrome">
        <section class="section">
          <div class="cli-health-heading">
            <div class="cli-health-disclosure">
              <button
                class="lattice-btn lattice-btn--ghost lattice-btn--menu-item lattice-btn--compact"
                aria-expanded={!cliHealthCollapsed}
                aria-controls="cli-health-list"
                onclick={() => cliHealthCollapsed = !cliHealthCollapsed}
              >
                <span class="chevron" class:collapsed={cliHealthCollapsed}>
                  {#if cliHealthCollapsed}
                    <CaretRight size={12} weight="light" />
                  {:else}
                    <CaretDown size={12} weight="light" />
                  {/if}
                </span>
                <h3 class="section-title">CLI Health</h3>
              </button>
            </div>
            <!-- Indeterminate CLI health-check action affordance, not content loading. -->
            <button
              class="lattice-btn lattice-btn--secondary lattice-btn--compact"
              class:lattice-btn--waiting={cliHealthLoading}
              onclick={() => void loadCliHealth(true)}
              disabled={cliHealthLoading}
              aria-busy={cliHealthLoading}
              title="Refresh CLI launch and authentication checks"
            >
              {cliHealthLoading ? 'Checking…' : 'Refresh'}
            </button>
          </div>
          {#if !cliHealthCollapsed}
            <div id="cli-health-list" class="cli-health-list" aria-live="polite">
              {#each cliOptions as cli}
                {@const health = cliHealth[cli.value]}
                <div class="cli-health-item lattice-panel">
                  <div class="cli-health-row">
                    <span class="cli-health-name">{cli.label}</span>
                    <span
                      class="cli-health-badge {cliHealthTone(health, cliHealthError)}"
                      title={health?.binPath
                        ? `${cliHealthMessage(health, cliHealthLoading, cliHealthError)} Executable: ${health.binPath}`
                        : cliHealthMessage(health, cliHealthLoading, cliHealthError)}
                    >
                      <span class="cli-health-dot" aria-hidden="true"></span>
                      {cliHealthLabel(health, cliHealthLoading, cliHealthError)}
                    </span>
                  </div>
                  <span class="cli-health-detail {cliHealthTone(health, cliHealthError)}">
                    {cliHealthMessage(health, cliHealthLoading, cliHealthError)}
                  </span>
                  {#if health?.binPath}
                    <span class="cli-health-path" title={health.binPath}>{health.binPath}</span>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        </section>

        {#if !$activeSession}
          <div class="empty-state">
            <p>No session selected</p>
            <p class="hint">Launch a new session to get started</p>
          </div>
        {:else}
        {#if isQaPhase($activeSession.state)}
          <section class="section">
            <QaFeedbackPanel />
          </section>
        {/if}

        <section class="section">
          <button class="lattice-btn lattice-btn--ghost lattice-btn--menu-item lattice-btn--compact" onclick={() => alertsCollapsed = !alertsCollapsed}>
            <span class="chevron" class:collapsed={alertsCollapsed}>
              {#if alertsCollapsed}
                <CaretRight size={12} weight="light" />
              {:else}
                <CaretDown size={12} weight="light" />
              {/if}
            </span>
            <h3 class="section-title">Alerts</h3>
          </button>
          {#if !alertsCollapsed}
            <div class="alerts">
              {#each $activeAgents.filter(a => typeof a.status === 'object' && 'WaitingForInput' in a.status) as agent}
                {@const lastLine = typeof agent.status === 'object' && 'WaitingForInput' in agent.status ? agent.status.WaitingForInput : ''}
                <button class="lattice-btn lattice-btn--warning lattice-btn--card lattice-btn--selected" onclick={() => handleAlertClick(agent.id)}>
                  <span class="alert-content">
                    <span class="alert-header">
                      <span class="alert-icon">
                        <Warning size={14} weight="fill" />
                      </span>
                      <span class="alert-title">{getAgentLabel(agent)} needs input</span>
                    </span>
                    {#if lastLine}
                      <span class="alert-body">
                        <span class="last-line">{lastLine}</span>
                      </span>
                    {/if}
                  </span>
                </button>
              {:else}
                <p class="no-alerts">No alerts</p>
              {/each}
            </div>
          {/if}
        </section>

        <section class="section">
          <button class="lattice-btn lattice-btn--ghost lattice-btn--menu-item lattice-btn--compact" onclick={() => infoCollapsed = !infoCollapsed}>
            <span class="chevron" class:collapsed={infoCollapsed}>
              {#if infoCollapsed}
                <CaretRight size={12} weight="light" />
              {:else}
                <CaretDown size={12} weight="light" />
              {/if}
            </span>
            <h3 class="section-title">Session Info</h3>
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
              {#if totalMilestones > 0}
                <div class="info-item">
                  <span class="info-label">Milestones</span>
                  <span class="info-value">{completedMilestones}/{totalMilestones}</span>
                </div>
              {/if}
            </div>
          {/if}
        </section>

        {#if isSessionActive($activeSession.state)}
          <section class="section actions-section">
            <div class="operator-controls">
              {#if serdeEnumVariantName($activeSession.state) === 'QaInProgress'}
                <button class="lattice-btn lattice-btn--secondary lattice-btn--compact" onclick={() => showSkipQaConfirm = true}>Skip QA</button>
              {/if}

              {#if canOverrideQa($activeSession.state, $activeSession.agents)}
                <div class="op-group">
                  <button class="lattice-btn lattice-btn--success lattice-btn--compact" onclick={() => showForcePassConfirm = true}>Force Pass</button>
                  <button class="lattice-btn lattice-btn--danger lattice-btn--compact" onclick={() => showForceFailConfirm = true}>Force Fail</button>
                </div>
              {/if}
            </div>

            {#if opError}
              <p class="op-error" role="alert">{opError}</p>
            {/if}

            <div class="close-session-action">
              <button
                class="lattice-btn lattice-btn--danger"
                onclick={() => showCloseConfirm = $activeSession?.id ?? null}
                title="Close this session (kills all agents and marks as closed)"
              >
                Close Session
              </button>
            </div>
          </section>
        {/if}
        {/if}
      </div>

      {#if $activeSession}
      <!-- Close confirmation dialog -->
      {#if showCloseConfirm}
        <div
          class="confirm-overlay lattice-modal-backdrop"
          onclick={dismissCloseConfirm}
          onkeydown={(event) => event.key === 'Escape' && dismissCloseConfirm()}
          role="presentation"
        >
          <div
            class="confirm-dialog lattice-modal"
            onclick={(e) => e.stopPropagation()}
            onkeydown={handleCloseDialogKeydown}
            role="dialog"
            aria-modal="true"
            tabindex="-1"
          >
            <h3>Close Session?</h3>
            <p>This will terminate all agents and mark the session as closed. This action cannot be undone.</p>
            <div class="confirm-actions">
              <button class="lattice-btn lattice-btn--secondary" onclick={dismissCloseConfirm} disabled={closing}>Cancel</button>
              <button class="lattice-btn lattice-btn--danger" onclick={handleCloseSession} disabled={closing}>
                {closing ? 'Closing...' : 'Close Session'}
              </button>
            </div>
          </div>
        </div>
      {/if}

      <!-- Force Fail confirmation dialog -->
      {#if showForceFailConfirm}
        <div
          class="confirm-overlay lattice-modal-backdrop"
          onclick={dismissForceFailConfirm}
          onkeydown={(event) => event.key === 'Escape' && dismissForceFailConfirm()}
          role="presentation"
        >
          <div
            class="confirm-dialog lattice-modal"
            onclick={(e) => e.stopPropagation()}
            onkeydown={handleForceFailDialogKeydown}
            role="dialog"
            aria-modal="true"
            tabindex="-1"
          >
            <h3>Force Fail Milestone?</h3>
            <p>This will immediately fail the current milestone and trigger a retry or termination. Are you sure?</p>
            <div class="confirm-actions">
              <button class="lattice-btn lattice-btn--secondary" onclick={dismissForceFailConfirm} disabled={forceFailing}>Cancel</button>
              <button class="lattice-btn lattice-btn--danger" onclick={handleForceFail} disabled={forceFailing}>
                {forceFailing ? 'Failing...' : 'Force Fail'}
              </button>
            </div>
          </div>
        </div>
      {/if}

      <!-- #176: Force Pass confirmation dialog. Previously a single unguarded click. -->
      {#if showForcePassConfirm}
        <div
          class="confirm-overlay lattice-modal-backdrop"
          onclick={() => (showForcePassConfirm = false)}
          onkeydown={(event) => event.key === 'Escape' && (showForcePassConfirm = false)}
          role="presentation"
        >
          <div
            class="confirm-dialog lattice-modal"
            onclick={(e) => e.stopPropagation()}
            onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Escape') showForcePassConfirm = false; }}
            role="dialog"
            aria-modal="true"
            tabindex="-1"
          >
            <h3>Force Pass Milestone?</h3>
            <p>This bypasses the Evaluator and marks QA as passed without review. Are you sure?</p>
            <div class="confirm-actions">
              <button class="lattice-btn lattice-btn--secondary" onclick={() => (showForcePassConfirm = false)}>Cancel</button>
              <button class="lattice-btn lattice-btn--success" onclick={handleForcePass}>Force Pass</button>
            </div>
          </div>
        </div>
      {/if}

      <!-- #176: Skip QA confirmation dialog (same destructive force-pass endpoint). -->
      {#if showSkipQaConfirm}
        <div
          class="confirm-overlay lattice-modal-backdrop"
          onclick={() => (showSkipQaConfirm = false)}
          onkeydown={(event) => event.key === 'Escape' && (showSkipQaConfirm = false)}
          role="presentation"
        >
          <div
            class="confirm-dialog lattice-modal"
            onclick={(e) => e.stopPropagation()}
            onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Escape') showSkipQaConfirm = false; }}
            role="dialog"
            aria-modal="true"
            tabindex="-1"
          >
            <h3>Skip QA?</h3>
            <p>This force-passes the current milestone without an Evaluator review. Are you sure?</p>
            <div class="confirm-actions">
              <button class="lattice-btn lattice-btn--secondary" onclick={() => (showSkipQaConfirm = false)}>Cancel</button>
              <button class="lattice-btn lattice-btn--warning" onclick={handleSkipQa}>Skip QA</button>
            </div>
          </div>
        </div>
      {/if}
    {/if}
</div>

<style>
  .status-content {
    position: relative;
    flex: 1;
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
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
    color: var(--text-secondary);
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

  .section-title {
    margin: 0;
    font-size: 11px;
    font-weight: 600;
    color: inherit;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .cli-health-heading {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .cli-health-disclosure {
    flex: 1;
    min-width: 0;
  }

  .cli-health-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .cli-health-item {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 7px 8px;
  }

  .cli-health-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .cli-health-name {
    min-width: 0;
    color: var(--text-primary);
    font-size: 11px;
    font-weight: 600;
  }

  .cli-health-badge {
    /* Executable-health pill is distinct from the session status-badge contract. */
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: 0 0 auto;
    padding: 2px 6px;
    box-shadow: inset 0 0 0 1px currentColor;
    border-radius: var(--radius-full);
    font-size: 9px;
    font-weight: 600;
    line-height: 1.2;
  }

  .cli-health-dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: currentColor;
  }

  .cli-health-badge.healthy,
  .cli-health-detail.healthy {
    color: var(--status-success);
  }

  .cli-health-badge.warning,
  .cli-health-detail.warning {
    color: var(--status-warning);
  }

  .cli-health-badge.error,
  .cli-health-detail.error {
    color: var(--status-error);
  }

  .cli-health-badge.pending,
  .cli-health-detail.pending {
    color: var(--text-disabled);
  }

  /* #176: operator-action failures were console-only before this. */
  .op-error {
    margin: 0.5rem 0 0;
    color: var(--status-error);
    font-size: 0.8rem;
    word-break: break-word;
  }

  .cli-health-detail,
  .cli-health-path {
    overflow: hidden;
    font-size: 9px;
    line-height: 1.35;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .cli-health-path {
    color: var(--text-secondary);
    font-family: var(--font-mono);
    opacity: 0.75;
  }

  .chevron {
    font-size: 8px;
    color: var(--text-secondary);
    transition: transform var(--motion-duration-standard) var(--motion-ease-standard);
  }

  .chevron.collapsed {
    transform: rotate(-90deg);
  }

  .alerts {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .alert-content {
    display: flex;
    flex-direction: column;
    width: 100%;
    gap: var(--space-1);
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
    display: block;
    font-size: 11px;
    font-family: var(--font-mono);
    opacity: 0.8;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text-primary);
  }

  .alert-icon {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .no-alerts {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary);
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
    color: var(--text-secondary);
  }

  .info-value {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary);
  }

  .info-value.state-running {
    color: var(--accent-cyan);
  }

  .info-value.state-completed {
    color: var(--status-success);
  }

  .info-value.state-failed {
    color: var(--status-error);
  }

  .info-value.state-closed {
    color: var(--text-secondary);
  }

  .actions-section {
    margin-top: auto;
    padding-top: 12px;
    box-shadow: var(--edge-seam-top);
  }

  .close-session-action {
    display: grid;
  }

  .confirm-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 10;
  }

  .confirm-dialog {
    padding: 20px;
    width: 220px;
    max-width: 90%;
  }

  .confirm-dialog h3 {
    margin: 0 0 12px 0;
    font-size: 15px;
    color: var(--text-primary);
  }

  .confirm-dialog p {
    margin: 0 0 16px 0;
    font-size: 12px;
    color: var(--text-secondary);
    line-height: 1.4;
  }

  .confirm-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

</style>
