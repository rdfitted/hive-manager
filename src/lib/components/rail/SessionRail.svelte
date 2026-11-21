<script lang="ts">
  import { Plus } from 'phosphor-svelte';
  import { heartbeatStore } from '$lib/stores/conversations';
  import { needsAttention, workStateFor } from '$lib/stores/agentActivity';
  import { serdeEnumVariantName, sessionDisplayName, type Session } from '$lib/stores/sessions';
  import { IconButton } from '../ui';

  let { sessions, activeId, onselect, oncreate }: {
    sessions: Session[];
    activeId?: string;
    onselect: (id: string) => void;
    oncreate: () => void;
  } = $props();

  const emptySeats = new Set<string>();

  function railAgentState(session: Session, index: number) {
    const agent = session.agents[index];
    if (!agent) return 'empty';
    const heartbeat = $heartbeatStore.agentsBySession?.[session.id]?.[agent.id];
    const stale = $heartbeatStore.staleAgentsBySession?.[session.id] ?? emptySeats;
    if (needsAttention(agent, stale, workStateFor(agent, heartbeat))) return 'attention';
    return serdeEnumVariantName(agent.status)?.toLowerCase() ?? 'empty';
  }
</script>

<nav class="sidebar-rail lattice-scroll-chrome" aria-label="Session rail">
  {#each sessions as session (session.id)}
    {#snippet sessionTile()}
      <span class="session-tile">
        {#each [0, 1, 2, 3] as index (index)}
          <span class="session-tile-dot" data-state={railAgentState(session, index)}></span>
        {/each}
      </span>
    {/snippet}
    {@const name = sessionDisplayName(session)}
    <IconButton size="sm" icon={sessionTile} label={name} tooltip={name} aria-current={activeId === session.id ? 'true' : undefined}
      pressed={activeId === session.id} onclick={() => onselect(session.id)} />
  {/each}
  <IconButton size="sm" icon={Plus} label="New Session" tooltip="New Session" onclick={oncreate} />
</nav>

<style>
  /* Floating glass rail: one tile per session, four cells for its first agents. */
  .sidebar-rail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    width: 56px;
    min-height: 0;
    max-height: calc(100% - var(--space-4));
    overflow-y: auto;
    margin: var(--space-1) 0 0 20px;
    padding: var(--space-2) 0;
    background: var(--sheen), var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-glass);
    box-shadow: var(--glass-shadow);
  }
  @supports ((backdrop-filter: blur(1px)) or (-webkit-backdrop-filter: blur(1px))) {
    .sidebar-rail {
      background: var(--sheen), var(--glass);
      -webkit-backdrop-filter: var(--glass-blur);
      backdrop-filter: var(--glass-blur);
    }
  }
  .sidebar-rail :global(.lattice-icon-button) {
    width: 40px;
    height: 40px;
    border-color: var(--line);
    border-radius: 9px;
    background: var(--well);
  }
  .sidebar-rail :global(.lattice-icon-button:hover:not(:disabled)) { border-color: var(--line-strong); background: var(--well); }
  .sidebar-rail :global(.lattice-icon-button[aria-pressed='true']) { border-color: var(--line-strong); background: var(--glass-strong); }
  .sidebar-rail :global(.lattice-icon-button:last-child) { border-style: dashed; border-color: var(--line-strong); background: transparent; }
  .session-tile { display: grid; grid-template: 1fr 1fr / 1fr 1fr; gap: 2px; width: 30px; height: 30px; }
  .session-tile-dot { border-radius: 2px; background: color-mix(in srgb, var(--ink) 12%, transparent); }
  .session-tile-dot[data-state='running'] { background: var(--tone-info); }
  .session-tile-dot[data-state='attention'] { background: var(--accent); }
  .session-tile-dot[data-state='completed'] { background: var(--tone-ok); opacity: .7; }
  .session-tile-dot[data-state='failed'], .session-tile-dot[data-state='error'] { background: var(--tone-err); }

  @media (forced-colors: active) {
    .sidebar-rail { border-color: CanvasText; background: Canvas; -webkit-backdrop-filter: none; backdrop-filter: none; box-shadow: none; }
    .session-tile-dot { background: ButtonText; forced-color-adjust: none; opacity: 1; }
    .session-tile-dot[data-state='empty'] { background: GrayText; }
    .session-tile-dot[data-state='attention'] { background: Highlight; }
  }
</style>
