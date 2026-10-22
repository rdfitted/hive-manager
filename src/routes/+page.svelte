<script lang="ts">
  import { onMount, untrack, tick } from 'svelte';
  import LaunchDialog from '$lib/components/LaunchDialog.svelte';
  import RightPanel from '$lib/components/RightPanel.svelte';
  import ShortcutsOverlay from '$lib/components/ShortcutsOverlay.svelte';
  import UpdateChecker from '$lib/components/UpdateChecker.svelte';
  import FusionPanel from '$lib/components/FusionPanel.svelte';
  import DebatePanel from '$lib/components/DebatePanel.svelte';
  import SessionOverview from '$lib/components/session/SessionOverview.svelte';
  import { readTerminalSelection } from '$lib/components/Terminal.svelte';
  import { sessions, activeSession, activeAgents, serdeEnumVariantName } from '$lib/stores/sessions';
  import { coordination } from '$lib/stores/coordination';
  import { ui } from '$lib/stores/ui';
  import { layout } from '$lib/stores/layout';
  import { pendingContext } from '$lib/stores/pendingContext';
  import { shell } from '$lib/stores/shell';
  import { launch } from '$lib/stores/launch';
  import { shortcutLabel } from '$lib/platform';

  let showShortcuts = $state(false);
  let landingVisible = $derived(!$activeSession || $launch.landingOpen);

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

  });

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
  function handleLandingEscape(event: KeyboardEvent) {
    if (event.key !== 'Escape' || !landingVisible || showShortcuts || document.querySelector('[aria-modal="true"]')) return;
    // The mounted grid also listens in capture phase; the landing owns this Escape.
    event.stopImmediatePropagation();
    if ($activeSession) {
      event.preventDefault();
      launch.close();
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    const mod = event.ctrlKey || event.metaKey;
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
        event.preventDefault();
        showShortcuts = false;
        return;
      }

      if (landingVisible) return;

      const modalWasOpen = document.querySelector('[aria-modal="true"]') !== null;
      queueMicrotask(() => {
        if (event.defaultPrevented || modalWasOpen) return;

        let collapsedPanel = false;
        if (!$layout.leftCollapsed) {
          layout.toggleLeft();
          collapsedPanel = true;
        }
        if ($activeSession && !$layout.rightCollapsed) {
          layout.toggleRight();
          collapsedPanel = true;
        }

        if (collapsedPanel) event.preventDefault();
      });
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
    const target = event.target instanceof HTMLElement ? event.target : null;
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

<svelte:window onkeydowncapture={handleLandingEscape} onkeydown={handleKeydown} />

<main class="main-content">
    <div class:welcome={landingVisible} class:lattice-scroll-content={landingVisible}>
      <LaunchDialog
        show={landingVisible}
        presentation="inline"
        launching={$launch.launching}
        launchError={$launch.launchError}
        initialMode={$launch.requestedMode}
        modeRequestId={$launch.requestId}
        defaultProjectPath={$launch.defaultProjectPath}
        on:close={() => launch.close()}
        on:launchHive={(event) => launch.launchHive(event.detail)}
        on:launchResearch={(event) => launch.launchResearch(event.detail)}
        on:launchFusion={(event) => launch.launchFusion(event.detail)}
        on:launchSolo={(event) => launch.launchSolo(event.detail)}
        on:launchDebate={(event) => launch.launchDebate(event.detail)}
      />
      {#if landingVisible}
        <div class="landing-links">
          <button type="button" class="open-recent lattice-btn lattice-btn--ghost" onclick={() => shell.requestStartAction('recent')}>Open recent</button>
          <p class="cta hint">Press <strong>{shortcutLabel('/')}</strong> for keyboard shortcuts</p>
        </div>
      {/if}
    </div>
    {#if $activeSession}
      <div class="terminal-area" hidden={landingVisible} inert={landingVisible} aria-hidden={landingVisible ? 'true' : undefined}>
        {#if $activeAgents.length === 0}
          <div class="no-agents">
            <section class="no-agents-content lattice-panel" aria-labelledby="no-agents-title">
              <h2 id="no-agents-title">No principals yet</h2>
              <p>Add a principal to begin working in this session.</p>
              <button type="button" class="lattice-btn lattice-btn--primary lattice-btn--filled" onclick={() => shell.openAddWorker()}>Add Principal</button>
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

<ShortcutsOverlay open={showShortcuts} onClose={() => showShortcuts = false} />
<UpdateChecker />

<style>
  .welcome {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: safe center;
    gap: var(--space-4);
    min-height: 0;
    padding: calc(var(--space-6) + var(--space-2));
    overflow-y: auto;
  }

  .landing-links {
    width: min(100%, calc(var(--space-7) * 10));
    text-align: center;
  }

  .open-recent {
    font-size: var(--text-small);
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
