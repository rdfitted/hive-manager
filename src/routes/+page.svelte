<script lang="ts">
  import { onMount, untrack, tick } from 'svelte';
  import { ArrowsSplit, ClockCounterClockwise, Crown, Scales } from 'phosphor-svelte';
  import SessionSidebar from '$lib/components/SessionSidebar.svelte';
  import RightPanel from '$lib/components/RightPanel.svelte';
  import AddWorkerDialog from '$lib/components/AddWorkerDialog.svelte';
  import ShortcutsOverlay from '$lib/components/ShortcutsOverlay.svelte';
  import UpdateChecker from '$lib/components/UpdateChecker.svelte';
  import FusionPanel from '$lib/components/FusionPanel.svelte';
  import DebatePanel from '$lib/components/DebatePanel.svelte';
  import SessionOverview from '$lib/components/session/SessionOverview.svelte';
  import { readTerminalSelection } from '$lib/components/Terminal.svelte';
  import { sessions, activeSession, activeAgents, serdeEnumVariantName, type HiveLaunchConfig, type FusionLaunchConfig, type DebateLaunchConfig } from '$lib/stores/sessions';
  import { coordination } from '$lib/stores/coordination';
  import { ui } from '$lib/stores/ui';
  import { layout } from '$lib/stores/layout';
  import { pendingContext } from '$lib/stores/pendingContext';

  let showAddWorkerDialog = $state(false);
  let showShortcuts = $state(false);
  let startAction = $state<{ id: number; action: 'hive' | 'fusion' | 'debate' | 'recent' } | null>(null);
  let startActionId = 0;

  // Use UI store as single source of truth for focused agent
  let focusedAgentId = $derived($ui.focusedAgentId);
  let activeSessionState = $derived(serdeEnumVariantName($activeSession?.state));

  onMount(() => {
    sessions.loadSessions();
  });

  // Track previous session ID and state to detect changes
  let prevSessionId: string | null = null;
  let prevSessionState: string | null = null;
  let isTransitioning = false;

  // Handle session changes and coordination loading
  $effect(() => {
    const session = $activeSession;
    const sessionId = session?.id ?? null;
    const sessionState = session ? serdeEnumVariantName(session.state) ?? null : null;

    if (sessionId && sessionId !== prevSessionId) {
      prevSessionId = sessionId;
      coordination.setSessionId(sessionId);
    }

    // Detect Planning -> Running transition to focus Queen
    if (sessionState === 'Running' && prevSessionState === 'Planning') {
      untrack(() => {
        if (!isTransitioning) {
          isTransitioning = true;
          tick().then(() => {
            const queen = $activeAgents.find(a => serdeEnumVariantName(a.role) === 'Queen' || a.id.endsWith('-queen'));
            if (queen) {
              ui.setFocusedAgent(queen.id);
              ui.setSelectedAgent(queen.id);
            }
            isTransitioning = false;
          });
        }
      });
    }

    prevSessionState = sessionState;
  });

  // Handle agent list changes - use untrack to avoid infinite loops
  $effect(() => {
    const agents = $activeAgents;

    // Read current focus ID without tracking it as a dependency
    const currentFocusId = untrack(() => $ui.focusedAgentId);

    // Auto-select first agent when agents are added and nothing is selected
    if (agents.length > 0 && !currentFocusId) {
      ui.setFocusedAgent(agents[0].id);
      ui.setSelectedAgent(agents[0].id);
      return;
    }

    // Reset if focused agent no longer exists
    if (currentFocusId && !agents.find(a => a.id === currentFocusId)) {
      const nextId = agents[0]?.id ?? null;
      ui.setFocusedAgent(nextId);
      ui.setSelectedAgent(nextId);
      return;
    }

    // Auto-focus agent requesting input
    const waitingAgent = agents.find(a => typeof a.status === 'object' && 'WaitingForInput' in a.status);
    if (waitingAgent && currentFocusId !== waitingAgent.id) {
      ui.setFocusedAgent(waitingAgent.id);
      ui.setSelectedAgent(waitingAgent.id);
    }
  });

  async function handleLaunchHiveV2(config: HiveLaunchConfig): Promise<void> {
    await sessions.launchHiveV2(config);
  }

  async function handleLaunchFusion(config: FusionLaunchConfig): Promise<void> {
    await sessions.launchFusion(config);
  }

  async function handleLaunchDebate(config: DebateLaunchConfig): Promise<void> {
    await sessions.launchDebate(config);
  }

  function openAddWorkerDialog() {
    showAddWorkerDialog = true;
  }

  function closeAddWorkerDialog() {
    showAddWorkerDialog = false;
  }

  function requestStartAction(action: 'hive' | 'fusion' | 'debate' | 'recent') {
    startAction = { id: ++startActionId, action };
  }

  // Read an xterm terminal selection if the focused/under-cursor element is inside one.
  function readXtermSelection(): string | null {
    return readTerminalSelection($ui.focusedAgentId);
  }

  // Capture the operator's current terminal/window text selection as one-shot context
  // for the next composer submit. CRLF is normalized and the text trimmed.
  function captureSelectionContext(sessionId: string) {
    const xtermText = readXtermSelection();
    const winText = window.getSelection()?.toString() ?? '';
    const raw = (xtermText ?? winText).replace(/\r\n/g, '\n').trim();

    if (raw) {
      pendingContext.capture({
        sessionId,
        agentId: $ui.focusedAgentId,
        kind: 'selection',
        text: raw,
        capturedAt: Date.now(),
      });
    }
  }

  // Keyboard shortcuts (Ctrl on Windows/Linux, Cmd on macOS)
  function handleKeydown(event: KeyboardEvent) {
    const mod = event.ctrlKey || event.metaKey;
    // Ctrl+B to toggle the left sidebar
    if (mod && event.key === 'b') {
      event.preventDefault();
      layout.toggleLeft();
    }
    // Ctrl+J to toggle the right panel
    if (mod && event.key === 'j') {
      event.preventDefault();
      layout.toggleRight();
    }
    // Ctrl+/ to toggle the shortcuts overlay; Esc closes it
    if (mod && event.key === '/') {
      event.preventDefault();
      showShortcuts = !showShortcuts;
    }
    if (event.key === 'Escape') {
      if (showShortcuts) {
        showShortcuts = false;
        return;
      }

      if (!event.defaultPrevented) {
        if (!$layout.leftCollapsed) {
          layout.toggleLeft();
        }
        if ($activeSession && !$layout.rightCollapsed) {
          layout.toggleRight();
        }
      }
    }
    // Ctrl+I: capture the terminal/window text selection as one-shot operator context for the
    // next composer submit. Skip when focus is inside the composer (don't hijack its own
    // selection). Reads xterm selection first, then the window selection.
    if (mod && (event.key === 'i' || event.key === 'I')) {
      const ctxTarget = event.target as HTMLElement | null;
      if (ctxTarget?.closest('[data-composer]')) return; // composer owns its selection
      const sid = $activeSession?.id ?? null;
      if (!sid) return;
      event.preventDefault();
      captureSelectionContext(sid);
    }
    // Navigate agents with arrow keys — skip when user is typing in inputs, textareas,
    // contenteditable regions, or terminal panes so we don't hijack their keystrokes.
    const target = event.target as HTMLElement | null;
    const inTypingContext = !!target && (
      target.tagName === 'INPUT' ||
      target.tagName === 'TEXTAREA' ||
      target.tagName === 'SELECT' ||
      target.isContentEditable ||
      !!target.closest('.xterm, .terminal, [data-terminal], [contenteditable="true"]')
    );
    if (!inTypingContext && $activeAgents.length > 0 && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) {
      const currentIndex = $activeAgents.findIndex(a => a.id === focusedAgentId);
      if (currentIndex !== -1) {
        event.preventDefault();
        const nextIndex = event.key === 'ArrowUp'
          ? Math.max(0, currentIndex - 1)
          : Math.min($activeAgents.length - 1, currentIndex + 1);
        ui.setFocusedAgent($activeAgents[nextIndex].id);
        ui.setSelectedAgent($activeAgents[nextIndex].id);
      }
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="app">
  <SessionSidebar
    onLaunchHiveV2={handleLaunchHiveV2}
    onLaunchFusion={handleLaunchFusion}
    onLaunchDebate={handleLaunchDebate}
    onOpenAddWorker={openAddWorkerDialog}
    {startAction}
  />

  <main class="main-content">
    {#if !$activeSession}
      <div class="welcome lattice-scroll-content">
        <section class="welcome-content" aria-labelledby="welcome-title">
          <h1 id="welcome-title">Hive Manager</h1>
          <p class="welcome-intro">Choose how you want to start.</p>
          <div class="features" role="group" aria-label="Start a session">
            <button type="button" class="feature lattice-btn lattice-btn--card lattice-panel" onclick={() => requestStartAction('hive')}>
              <span class="feature-icon" aria-hidden="true">
                <Crown size={24} weight="light" />
              </span>
              <span class="feature-copy">
                <span class="feature-title">Hive</span>
                <span class="feature-text">Coordinate implementation with a principal-led team.</span>
              </span>
            </button>
            <button type="button" class="feature lattice-btn lattice-btn--card lattice-panel" onclick={() => requestStartAction('fusion')}>
              <span class="feature-icon" aria-hidden="true">
                <ArrowsSplit size={24} weight="light" />
              </span>
              <span class="feature-copy">
                <span class="feature-title">Fusion</span>
                <span class="feature-text">Compare independent approaches and synthesize the best.</span>
              </span>
            </button>
            <button type="button" class="feature lattice-btn lattice-btn--card lattice-panel" onclick={() => requestStartAction('debate')}>
              <span class="feature-icon" aria-hidden="true">
                <Scales size={24} weight="light" />
              </span>
              <span class="feature-copy">
                <span class="feature-title">Debate</span>
                <span class="feature-text">Test a decision through structured opposing arguments.</span>
              </span>
            </button>
            <button type="button" class="feature lattice-btn lattice-btn--card lattice-panel" onclick={() => requestStartAction('recent')}>
              <span class="feature-icon" aria-hidden="true">
                <ClockCounterClockwise size={24} weight="light" />
              </span>
              <span class="feature-copy">
                <span class="feature-title">Open recent</span>
                <span class="feature-text">Resume a stored session from this project.</span>
              </span>
            </button>
          </div>
          <p class="cta hint">Press <strong>Ctrl+/</strong> for keyboard shortcuts</p>
        </section>
      </div>
    {:else}
      <div class="terminal-area">
        {#if $activeAgents.length === 0}
          <div class="no-agents">
            <section class="no-agents-content lattice-panel" aria-labelledby="no-agents-title">
              <h2 id="no-agents-title">No principals yet</h2>
              <p>Add a principal to begin working in this session.</p>
              <button type="button" class="lattice-btn lattice-btn--primary lattice-btn--filled" onclick={openAddWorkerDialog}>Add Principal</button>
            </section>
          </div>
        {:else if $activeSession?.session_type && 'Fusion' in $activeSession.session_type && activeSessionState !== 'Planning' && activeSessionState !== 'PlanReady'}
          <FusionPanel />
        {:else if $activeSession?.session_type && 'Debate' in $activeSession.session_type && activeSessionState !== 'Planning' && activeSessionState !== 'PlanReady'}
          <DebatePanel />
        {:else}
          <SessionOverview />
        {/if}
      </div>
    {/if}
  </main>

  {#if $activeSession}
    <RightPanel />
  {/if}
</div>

<AddWorkerDialog bind:open={showAddWorkerDialog} on:close={closeAddWorkerDialog} />
<ShortcutsOverlay open={showShortcuts} onClose={() => showShortcuts = false} />
<UpdateChecker />

<style>
  .welcome {
    flex: 1;
    display: flex;
    align-items: safe center;
    justify-content: center;
    min-height: 0;
    padding: calc(var(--space-6) + var(--space-2));
    overflow-y: auto;
  }

  .welcome-content {
    width: min(100%, calc(var(--space-7) * 10));
    text-align: center;
  }

  .welcome h1 {
    margin: 0 0 var(--space-3);
    font-family: var(--font-display);
    font-size: var(--text-h1);
    font-weight: 700;
    color: var(--text-primary);
  }

  .welcome-intro {
    margin: 0 0 var(--space-6);
    font-size: var(--text-h3);
    color: var(--text-secondary);
  }

  .features {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    margin-bottom: var(--space-6);
  }

  .feature {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-4) calc(var(--space-4) + var(--space-1));
    background: var(--bg-panel);
    box-shadow: var(--elev-1), var(--edge-lip);
    text-align: left;
    transition:
      background-color var(--motion-duration-fast) var(--motion-ease-standard),
      box-shadow var(--motion-duration-fast) var(--motion-ease-standard);
  }

  .feature:hover:where(:not(:disabled):not([aria-disabled="true"])) {
    background: var(--bg-raised);
    box-shadow: var(--elev-2), var(--edge-lip);
  }

  .feature-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--accent-cyan);
  }

  .feature-copy {
    display: flex;
    min-width: 0;
    flex-direction: column;
    gap: var(--space-1);
  }

  .feature-title {
    font-family: var(--font-display);
    font-size: var(--text-h3);
    font-weight: 600;
    color: var(--text-primary);
  }

  .feature-text {
    font-size: var(--text-base);
    color: var(--text-secondary);
  }

  .cta {
    margin: 0;
    font-size: var(--text-base);
    color: var(--text-secondary);
  }

  .cta strong {
    color: var(--accent-cyan);
  }

  .cta.hint {
    margin-top: var(--space-2);
    font-size: var(--text-small);
    color: var(--text-disabled);
  }

  .terminal-area {
    flex: 1;
    position: relative;
    padding: var(--space-4);
    overflow: hidden;
  }

  .no-agents {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 100%;
    padding: var(--space-6);
    color: var(--text-secondary);
  }

  .no-agents-content {
    width: min(100%, calc(var(--space-7) * 8));
    padding: var(--space-6);
    text-align: center;
  }

  .no-agents-content h2 {
    margin: 0 0 var(--space-3);
    font-family: var(--font-display);
    font-size: var(--text-h2);
    color: var(--text-primary);
  }

  .no-agents-content p {
    margin: 0 0 var(--space-5);
    font-size: var(--text-base);
  }
</style>
