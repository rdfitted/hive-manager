<script lang="ts">
  import { onMount, untrack, tick } from 'svelte';
  import { ChartBar, ChatCenteredText, Crown, GearSix } from 'phosphor-svelte';
  import SessionSidebar from '$lib/components/SessionSidebar.svelte';
  import RightPanel from '$lib/components/RightPanel.svelte';
  import AddWorkerDialog from '$lib/components/AddWorkerDialog.svelte';
  import ShortcutsOverlay from '$lib/components/ShortcutsOverlay.svelte';
  import UpdateChecker from '$lib/components/UpdateChecker.svelte';
  import FusionPanel from '$lib/components/FusionPanel.svelte';
  import DebatePanel from '$lib/components/DebatePanel.svelte';
  import SessionOverview from '$lib/components/session/SessionOverview.svelte';
  import { readTerminalSelection } from '$lib/components/Terminal.svelte';
  import { sessions, activeSession, activeAgents, serdeEnumVariantName, type HiveLaunchConfig, type SwarmLaunchConfig, type FusionLaunchConfig, type DebateLaunchConfig } from '$lib/stores/sessions';
  import { coordination } from '$lib/stores/coordination';
  import { ui } from '$lib/stores/ui';
  import { layout } from '$lib/stores/layout';
  import { pendingContext } from '$lib/stores/pendingContext';

  let showAddWorkerDialog = $state(false);
  let showShortcuts = $state(false);

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

  async function handleLaunch(projectPath: string, workerCount: number, command: string, prompt?: string): Promise<void> {
    await sessions.launchHive(projectPath, workerCount, command, prompt);
  }

  async function handleLaunchHiveV2(config: HiveLaunchConfig): Promise<void> {
    await sessions.launchHiveV2(config);
  }

  async function handleLaunchSwarm(config: SwarmLaunchConfig): Promise<void> {
    await sessions.launchSwarm(config);
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

  // Read an xterm terminal selection if the focused/under-cursor element is inside one.
  function readXtermSelection(): string | null {
    return readTerminalSelection($ui.focusedAgentId);
  }

  // Capture the operator's current selection (terminal or page) as a one-shot context
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
    if (event.key === 'Escape' && showShortcuts) {
      showShortcuts = false;
    }
    // Ctrl+I: capture the active selection / cell as one-shot operator context for the
    // next composer submit. Skip when focus is inside the composer (don't hijack its own
    // selection). Reads xterm selection first, then the window selection, else the
    // selected session cell.
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
    onLaunch={handleLaunch}
    onLaunchHiveV2={handleLaunchHiveV2}
    onLaunchSwarm={handleLaunchSwarm}
    onLaunchFusion={handleLaunchFusion}
    onLaunchDebate={handleLaunchDebate}
    onOpenAddWorker={openAddWorkerDialog}
  />

  <main class="main-content">
    {#if !$activeSession}
      <div class="welcome">
        <div class="welcome-content">
          <h1>Hive Manager</h1>
          <p>Orchestrate and monitor Claude Code multi-agent workflows</p>
          <div class="features">
            <div class="feature">
              <span class="feature-icon">
                <Crown size={24} weight="light" />
              </span>
              <span class="feature-text">Launch Hive or Swarm sessions with hierarchical agents</span>
            </div>
            <div class="feature">
              <span class="feature-icon">
                <GearSix size={24} weight="light" />
              </span>
              <span class="feature-text">Configure each agent with different commands</span>
            </div>
            <div class="feature">
              <span class="feature-icon">
                <ChartBar size={24} weight="light" />
              </span>
              <span class="feature-text">Monitor agent status in real-time</span>
            </div>
            <div class="feature">
              <span class="feature-icon">
                <ChatCenteredText size={24} weight="light" />
              </span>
              <span class="feature-text">Interact with agents directly</span>
            </div>
          </div>
          <p class="cta">Click <strong>New Session</strong> in the sidebar to get started</p>
          <p class="cta hint">Press <strong>Ctrl+/</strong> for keyboard shortcuts</p>
        </div>
      </div>
    {:else}
      <div class="terminal-area">
        {#if $activeAgents.length === 0}
          <div class="no-agents">
            <p>No agents in this session</p>
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
  :global(*) {
    box-sizing: border-box;
  }

  :global(body) {
    margin: 0;
    padding: 0;
    overflow: hidden;
  }

  .app {
    display: flex;
    width: 100vw;
    height: 100vh;
    background: var(--color-bg);
    color: var(--color-text);
    font-family: var(--font-body);
  }

  .main-content {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow: hidden;
  }

  .welcome {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 40px;
  }

  .welcome-content {
    max-width: 500px;
    text-align: center;
  }

  .welcome h1 {
    margin: 0 0 12px 0;
    font-size: 32px;
    font-weight: 700;
    color: var(--color-text);
  }

  .welcome p {
    margin: 0 0 32px 0;
    font-size: 16px;
    color: var(--color-text-muted);
  }

  .features {
    display: flex;
    flex-direction: column;
    gap: 16px;
    margin-bottom: 32px;
  }

  .feature {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 20px;
    background: var(--color-surface);
    border-radius: 8px;
    text-align: left;
  }

  .feature-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .feature-text {
    font-size: 14px;
    color: var(--color-text);
  }

  .cta {
    font-size: 14px;
    color: var(--color-text-muted);
  }

  .cta strong {
    color: var(--color-accent);
  }

  .cta.hint {
    margin-top: 8px;
    font-size: 12px;
    opacity: 0.8;
  }

  .terminal-area {
    flex: 1;
    position: relative;
    padding: 16px;
    overflow: hidden;
  }

  .no-agents {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--color-text-muted);
  }
</style>
