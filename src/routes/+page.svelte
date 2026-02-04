<script lang="ts">
  import { onMount } from 'svelte';
  import Terminal from '$lib/components/Terminal.svelte';
  import SessionSidebar from '$lib/components/SessionSidebar.svelte';
  import StatusPanel from '$lib/components/StatusPanel.svelte';
  import { sessions, activeSession, activeAgents } from '$lib/stores/sessions';

  let showStatusPanel = $state(true);

  onMount(() => {
    sessions.loadSessions();
  });

  async function handleLaunch(projectPath: string, workerCount: number, prompt?: string): Promise<void> {
    await sessions.launchHive(projectPath, workerCount, prompt);
  }

  function toggleStatusPanel() {
    showStatusPanel = !showStatusPanel;
  }

  // Keyboard shortcuts
  function handleKeydown(event: KeyboardEvent) {
    // Ctrl+J to toggle status panel
    if (event.ctrlKey && event.key === 'j') {
      event.preventDefault();
      toggleStatusPanel();
    }
    // Ctrl+N for new session
    if (event.ctrlKey && event.key === 'n') {
      event.preventDefault();
      // Focus the new session button - handled by SessionSidebar
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="app">
  <SessionSidebar onLaunch={handleLaunch} />

  <main class="main-content">
    {#if !$activeSession}
      <div class="welcome">
        <div class="welcome-content">
          <h1>Hive Manager</h1>
          <p>Orchestrate and monitor Claude Code multi-agent workflows</p>
          <div class="features">
            <div class="feature">
              <span class="feature-icon">🐝</span>
              <span class="feature-text">Launch Hive sessions with multiple workers</span>
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
        {:else if $activeAgents.length === 1}
          <div class="single-terminal">
            <div class="terminal-header">
              <span class="terminal-title">
                {$activeAgents[0].role === 'Queen' ? 'Queen' :
                 typeof $activeAgents[0].role === 'object' && 'Planner' in $activeAgents[0].role ? `Planner ${$activeAgents[0].role.Planner.index}` :
                 typeof $activeAgents[0].role === 'object' && 'Worker' in $activeAgents[0].role ? `Worker ${$activeAgents[0].role.Worker.index}` :
                 'Agent'}
              </span>
            </div>
            <div class="terminal-container">
              <Terminal agentId={$activeAgents[0].id} />
            </div>
          </div>
        {:else}
          <div class="terminal-grid" class:two={$activeAgents.length === 2} class:four={$activeAgents.length >= 3}>
            {#each $activeAgents.slice(0, 4) as agent}
              <div class="terminal-panel">
                <div class="terminal-header">
                  <span class="terminal-title">
                    {agent.role === 'Queen' ? 'Queen' :
                     typeof agent.role === 'object' && 'Planner' in agent.role ? `Planner ${agent.role.Planner.index}` :
                     typeof agent.role === 'object' && 'Worker' in agent.role ? `Worker ${agent.role.Worker.index}` :
                     'Agent'}
                  </span>
                  <span class="terminal-status" class:running={agent.status === 'Running'} class:waiting={agent.status === 'WaitingForInput'} class:completed={agent.status === 'Completed'}>
                    {agent.status === 'Running' ? '█' : agent.status === 'WaitingForInput' ? '⏳' : agent.status === 'Completed' ? '✓' : '○'}
                  </span>
                </div>
                <div class="terminal-container">
                  <Terminal agentId={agent.id} />
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </main>

  {#if showStatusPanel}
    <StatusPanel />
  {/if}
</div>

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
    display: flex;
    flex-direction: column;
    padding: 16px;
    gap: 16px;
    overflow: hidden;
  }

  .no-agents {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--color-text-muted);
  }

  .single-terminal {
    flex: 1;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--color-border);
    border-radius: 8px;
    overflow: hidden;
  }

  .terminal-grid {
    flex: 1;
    display: grid;
    gap: 16px;
    overflow: hidden;
  }

  .terminal-grid.two {
    grid-template-columns: 1fr 1fr;
  }

  .terminal-grid.four {
    grid-template-columns: 1fr 1fr;
    grid-template-rows: 1fr 1fr;
  }

  .terminal-panel {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--color-border);
    border-radius: 8px;
    overflow: hidden;
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
</style>
