<script lang="ts">
  import { setContext, type Snippet } from 'svelte';
  import { paneSplitContext, paneStatusLineContext, type PaneMode } from './paneMode';

  let { mode, terminal, chat, statusLine }: {
    mode: PaneMode;
    terminal: Snippet;
    chat: Snippet;
    statusLine?: Snippet;
  } = $props();

  const terminalHidden = $derived(mode === 'chat' || mode === 'raw');
  const chatHidden = $derived(mode === 'terminal');
  setContext(paneStatusLineContext, () => mode === 'chat' ? statusLine : undefined);
  setContext(paneSplitContext, () => mode === 'split');
</script>

<div class="agent-pane-body" data-mode={mode}>
  <div class="chat-layer" class:pane-hidden={chatHidden} inert={chatHidden} aria-hidden={chatHidden}>
    {@render chat()}
  </div>
  <div class="terminal-layer" class:chat-hidden={terminalHidden} inert={terminalHidden} aria-hidden={terminalHidden}>
    {@render terminal()}
  </div>
</div>

<style>
  .agent-pane-body {
    display: grid;
    grid-template: 'body' minmax(0, 1fr) / minmax(0, 1fr);
    width: 100%;
    height: 100%;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .chat-layer, .terminal-layer { grid-area: body; min-width: 0; min-height: 0; }

  .agent-pane-body[data-mode='split'] {
    grid-template: 'chat' minmax(0, 1fr) 'terminal' min(11rem, 40%) / minmax(0, 1fr);
  }

  [data-mode='split'] .chat-layer { grid-area: chat; }
  [data-mode='split'] .terminal-layer { grid-area: terminal; }
  .pane-hidden, .chat-hidden { visibility: hidden; pointer-events: none; }

  /* Keep the terminal measurable, with its native drop rectangle outside the pane. */
  .chat-hidden { transform: translateX(-200vw); }

  [data-mode='split'] .terminal-layer { border-top: 1px solid var(--line); }

  /* Where anchor positioning exists, the drawer sits between the transcript and
     the composer, as specced: the chat leaves a slot there and the pane's one
     terminal layer is laid over it. Elsewhere it stays below the composer. */
  @supports (anchor-name: --pane-terminal-slot) and (anchor-scope: --pane-terminal-slot) {
    .agent-pane-body[data-mode='split'] {
      position: relative;
      grid-template: 'body' minmax(0, 1fr) / minmax(0, 1fr);
      anchor-scope: --pane-terminal-slot;
    }
    [data-mode='split'] .chat-layer { grid-area: body; }
    [data-mode='split'] .terminal-layer {
      position: absolute;
      position-anchor: --pane-terminal-slot;
      inset: anchor(top) anchor(right) anchor(bottom) anchor(left);
    }
  }
</style>
