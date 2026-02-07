<script lang="ts">
  import { onMount, untrack, tick } from 'svelte';
  import Terminal from '$lib/components/Terminal.svelte';
  import TerminalGrid from '$lib/components/TerminalGrid.svelte';
  import SessionSidebar from '$lib/components/SessionSidebar.svelte';
  import StatusPanel from '$lib/components/StatusPanel.svelte';
  import AgentTree from '$lib/components/AgentTree.svelte';
  import RightDrawer from '$lib/components/RightDrawer.svelte';
  import QueenControls from '$lib/components/QueenControls.svelte';
  import AddWorkerDialog from '$lib/components/AddWorkerDialog.svelte';
  import UpdateChecker from '$lib/components/UpdateChecker.svelte';
  import FusionPanel from '$lib/components/FusionPanel.svelte';
  import { sessions, activeSession, activeAgents, type HiveLaunchConfig, type SwarmLaunchConfig, type FusionLaunchConfig } from '$lib/stores/sessions';
  import { coordination } from '$lib/stores/coordination';
  import { ui } from '$lib/stores/ui';

  let showStatusPanel = $state(true);
  let showCoordinationPanel = $state(true);
  let showAddWorkerDialog = $state(false);
  let hierarchyCollapsed = $state(true);

  // Use UI store as single source of truth for focused agent
  let focusedAgentId = $derived($ui.focusedAgentId);

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
    const sessionState = session ? (typeof session.state === 'string' ? session.state : Object.keys(session.state)[0]) : null;

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
            const queen = $activeAgents.find(a => a.role === 'Queen' || a.id.endsWith('-queen'));
            if (queen) {
              ui.setFocusedAgent(queen.id);
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
      return;
    }

    // Reset if focused agent no longer exists
    if (currentFocusId && !agents.find(a => a.id === currentFocusId)) {
      ui.setFocusedAgent(agents[0]?.id ?? null);
      return;
    }

    // Auto-focus agent requesting input
    const waitingAgent = agents.find(a => typeof a.status === 'object' && 'WaitingForInput' in a.status);
    if (waitingAgent && currentFocusId !== waitingAgent.id) {
      ui.setFocusedAgent(waitingAgent.id);
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

  function toggleStatusPanel() {
    showStatusPanel = !showStatusPanel;
  }

  function toggleCoordinationPanel() {
    showCoordinationPanel = !showCoordinationPanel;
  }

  function openAddWorkerDialog() {
    showAddWorkerDialog = true;
  }

  function closeAddWorkerDialog() {
    showAddWorkerDialog = false;
  }

  function handleAgentSelect(e: CustomEvent<string>) {
    focusedAgentId = e.detail;
  }

  // Keyboard shortcuts
  function handleKeydown(event: KeyboardEvent) {
    // Ctrl+J to toggle status panel
    if (event.ctrlKey && event.key === 'j') {
      event.preventDefault();
      toggleStatusPanel();
    }
    // Ctrl+K to toggle coordination panel
    if (event.ctrlKey && event.key === 'k') {
      event.preventDefault();
      toggleCoordinationPanel();
    }
    // Ctrl+N for new session
    if (event.ctrlKey && event.key === 'n') {
      event.preventDefault();
      // Focus the new session button - handled by SessionSidebar
    }
    // Ctrl+G to toggle grid layout
    if (event.ctrlKey && event.key === 'g') {
      event.preventDefault();
      ui.toggleLayoutMode();
    }
    // Navigate agents with arrow keys when tree is focused
    if ($activeAgents.length > 0 && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) {
      const currentIndex = $activeAgents.findIndex(a => a.id === focusedAgentId);
      if (currentIndex !== -1) {
        event.preventDefault();
        const nextIndex = event.key === 'ArrowUp'
          ? Math.max(0, currentIndex - 1)
          : Math.min($activeAgents.length - 1, currentIndex + 1);
        focusedAgentId = $activeAgents[nextIndex].id;
      }
    }
  }

  let focusedAgent = $derived($activeAgents.find(a => a.id === focusedAgentId));
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="app">
  <SessionSidebar
    onLaunch={handleLaunch}
    onLaunchHiveV2={handleLaunchHiveV2}
    onLaunchSwarm={handleLaunchSwarm}
    onLaunchFusion={handleLaunchFusion}
  />

  {#if $activeSession}
    <aside class="hierarchy-sidebar" class:collapsed={hierarchyCollapsed}>
      <button class="sidebar-header" onclick={() => hierarchyCollapsed = !hierarchyCollapsed} title={hierarchyCollapsed ? "Expand Hierarchy" : "Collapse Hierarchy"}>
        <span class="sidebar-icon">🌳</span>
        {#if !hierarchyCollapsed}
          <h2>Hierarchy</h2>
        {/if}
      </button>
      {#if !hierarchyCollapsed}
        <div class="sidebar-content">
          <AgentTree
            agents={$activeAgents}
            selectedId={focusedAgentId}
            on:select={handleAgentSelect}
          />
        </div>
        <div class="queen-controls-section">
          <QueenControls on:openAddWorker={openAddWorkerDialog} />
        </div>
      {/if}
    </aside>
  {/if}

  <main class="main-content">
    {#if !$activeSession}
      <div class="welcome">
        <div class="welcome-content">
          <h1>Hive Manager</h1>
          <p>Orchestrate and monitor Claude Code multi-agent workflows</p>
          <div class="features">
            <div class="feature">
              <span class="feature-icon">♕</span>
              <span class="feature-text">Launch Hive or Swarm sessions with hierarchical agents</span>
            </div>
            <div class="feature">
              <span class="feature-icon">⚙</span>
              <span class="feature-text">Configure each agent with different commands</span>
            </div>
            <div class="feature">
              <span class="feature-icon">📊</span>
              <span class="feature-text">Monitor agent status in real-time</span>
            </div>
            <div class="feature">
              <span class="feature-icon">💬</span>
              <span class="feature-text">Interact with agents directly</span>
            </div>
          </div>
          <p class="cta">Click <strong>New Session</strong> in the sidebar to get started</p>
        </div>
      </div>
    {:else}
      <div class="terminal-area">
        {#if $activeAgents.length === 0}
          <div class="no-agents">
            <p>No agents in this session</p>
          </div>
        {:else if $activeSession?.session_type && 'Fusion' in $activeSession.session_type}
          <FusionPanel />
        {:else if $ui.layoutMode === 'grid'}
          <TerminalGrid
            agents={$activeAgents}
            {focusedAgentId}
            onSelect={(id) => focusedAgentId = id}
          />
        {:else}
          <!-- Render all terminals, show only the focused one -->
          {#each $activeAgents as agent (agent.id)}
            {@const isVisible = agent.id === focusedAgentId}
            {@const roleName = agent.config?.label ||
              (agent.role === 'Queen' ? 'Queen' :
               typeof agent.role === 'object' && 'Planner' in agent.role ? `Planner ${agent.role.Planner.index}` :
               typeof agent.role === 'object' && 'Worker' in agent.role ? `Worker ${agent.role.Worker.index}` :
               'Agent')}
            <div class="focused-terminal" class:hidden={!isVisible}>
              <div class="terminal-header">
                <span class="terminal-title">{roleName}</span>
                <div class="terminal-meta">
                  <span class="cli-badge">{agent.config?.cli || 'unknown'}</span>
                  <span class="terminal-status" 
                    class:running={agent.status === 'Running'} 
                    class:waiting={typeof agent.status === 'object' && 'WaitingForInput' in agent.status} 
                    class:completed={agent.status === 'Completed'}
                  >
                    {agent.status === 'Running' ? '█' : 
                     (typeof agent.status === 'object' && 'WaitingForInput' in agent.status) ? '⏳' : 
                     agent.status === 'Completed' ? '✓' : '○'}
                  </span>
                </div>
              </div>
              <div class="terminal-container">
                <Terminal agentId={agent.id} isFocused={isVisible} />
              </div>
            </div>
          {/each}
        {/if}
      </div>
    {/if}
  </main>

  {#if showStatusPanel}
    <StatusPanel />
  {/if}

  {#if showCoordinationPanel && $activeSession}
    <aside class="coordination-sidebar">
      <RightDrawer />
    </aside>
  {/if}
</div>

<AddWorkerDialog bind:open={showAddWorkerDialog} on:close={closeAddWorkerDialog} />
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

  :global(:root) {
    /* Tokyo Night theme */
    --color-bg: #1a1b26;
    --color-surface: #24283b;
    --color-surface-hover: #2f3549;
    --color-border: #414868;
    --color-text: #c0caf5;
    --color-text-muted: #565f89;
    --color-accent: #7aa2f7;
    --color-accent-bright: #89b4fa;
    --color-accent-dim: rgba(122, 162, 247, 0.15);
    --color-primary: #8b5cf6;
    --color-primary-muted: rgba(139, 92, 246, 0.15);
    --color-success: #9ece6a;
    --color-warning: #e0af68;
    --color-error: #f7768e;
    --color-running: #7aa2f7;
  }

  .app {
    display: flex;
    width: 100vw;
    height: 100vh;
    background: var(--color-bg);
    color: var(--color-text);
    font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
  }

  .hierarchy-sidebar {
    width: 200px;
    min-width: 200px;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--color-surface);
    border-right: 1px solid var(--color-border);
    transition: width 0.2s ease, min-width 0.2s ease;
  }

  .hierarchy-sidebar.collapsed {
    width: 52px;
    min-width: 52px;
  }

  .hierarchy-sidebar .sidebar-header {
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

  .hierarchy-sidebar .sidebar-header:hover {
    background: var(--color-surface-hover);
  }

  .hierarchy-sidebar .sidebar-icon {
    font-size: 18px;
    flex-shrink: 0;
  }

  .hierarchy-sidebar .sidebar-header h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--color-text);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    white-space: nowrap;
  }

  .hierarchy-sidebar .sidebar-content {
    flex: 1;
    overflow-y: auto;
    padding: 8px;
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
    font-size: 24px;
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

  .focused-terminal {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--color-border);
    border-radius: 8px;
    overflow: hidden;
  }

  .focused-terminal.hidden {
    visibility: hidden;
    pointer-events: none;
  }

  .terminal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    background: var(--color-surface);
    border-bottom: 1px solid var(--color-border);
  }

  .terminal-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--color-text);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .terminal-meta {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .cli-badge {
    font-size: 10px;
    padding: 2px 6px;
    background: var(--color-border);
    border-radius: 3px;
    color: var(--color-text-muted);
    text-transform: lowercase;
  }

  .terminal-status {
    font-size: 10px;
  }

  .terminal-status.running {
    color: var(--color-running);
  }

  .terminal-status.waiting {
    color: var(--color-warning);
  }

  .terminal-status.completed {
    color: var(--color-success);
  }

  .terminal-container {
    flex: 1;
    min-height: 0;
  }

  .queen-controls-section {
    border-top: 1px solid var(--color-border);
    padding: 8px;
  }

  .coordination-sidebar {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--color-surface);
    border-left: 1px solid var(--color-border);
  }

  .coordination-sidebar :global(.right-drawer) {
    width: 320px;
    min-width: 320px;
    transition: width 0.2s ease, min-width 0.2s ease;
  }

  .coordination-sidebar :global(.right-drawer.collapsed) {
    width: 52px;
    min-width: 52px;
  }
</style>
