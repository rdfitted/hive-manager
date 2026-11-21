<script lang="ts">
  import { GitBranch } from 'phosphor-svelte';
  import { serdeEnumVariantName, type Session } from '$lib/stores/sessions';

  let { session }: { session: Session } = $props();
  const state = $derived(serdeEnumVariantName(session.state) ?? 'Unknown');
  const branch = $derived(session.worktree_branch?.trim());
  const worktree = $derived(session.worktree_path?.trim());
  const statusColors: Record<string, string> = {
    running: 'var(--status-running)', active: 'var(--status-running)',
    starting: 'var(--status-running)', initializing: 'var(--status-running)',
    completed: 'var(--status-success)', closed: 'var(--status-success)',
    failed: 'var(--status-error)', error: 'var(--status-error)',
    blocked: 'var(--status-blocked)', closing: 'var(--status-warning)',
  };
</script>

<span class="session-state" data-tauri-drag-region data-window-chrome
  role="img" aria-label={`Session state: ${state}`} title={`Session state: ${state}`}
  style:--state-color={statusColors[state.toLowerCase()] ?? 'var(--ink-muted)'}>
  <span class="session-state-dot" aria-hidden="true" data-tauri-drag-region data-window-chrome></span>
  {state}
</span>
{#if branch || worktree}
  <span class="worktree-chip" data-tauri-drag-region data-window-chrome title={worktree ?? branch}>
    <GitBranch size={14} aria-hidden="true" />
    <span class="worktree-label" data-tauri-drag-region data-window-chrome>{branch || worktree}</span>
  </span>
{/if}

<style>
  /* A dot leads; the word stays beside it so the state never depends on colour or hover. */
  .session-state {
    display: inline-flex;
    align-items: center;
    flex: none;
    gap: 7px;
    padding: 4px 2px;
    color: var(--ink-dim);
    font-size: 11px;
    white-space: nowrap;
  }
  .session-state-dot {
    flex: none;
    width: 7px;
    height: 7px;
    border: 1px solid var(--state-color);
    border-radius: var(--radius-full);
    background: var(--state-color);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--state-color) 18%, transparent);
  }
  .worktree-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    min-width: 0;
    color: var(--ink-dim);
    font: 11px/1 var(--font-mono);
  }
  .worktree-chip :global(svg) { flex: none; pointer-events: none; }
  .worktree-label { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }

  @media (forced-colors: active) {
    .session-state-dot { border-color: CanvasText; box-shadow: none; }
  }
</style>
