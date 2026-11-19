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
  .sidebar-rail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    min-height: 0;
    max-height: 100%;
    overflow-y: auto;
    margin: var(--space-1);
    padding: var(--space-2) var(--space-1);
    background: var(--surface);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-lg);
  }
  .session-tile { display: grid; grid-template-columns: repeat(2, var(--space-1)); gap: var(--space-1); }
  .session-tile-dot { height: var(--space-1); background: var(--border-control); border-radius: var(--radius-full); }
  .session-tile-dot[data-state='running'] { background: var(--status-running); }
  .session-tile-dot[data-state='attention'] { background: var(--status-warning); }
  .session-tile-dot[data-state='completed'] { background: var(--status-success); }
  .session-tile-dot[data-state='failed'] { background: var(--status-error); }

  @media (forced-colors: active) {
    .sidebar-rail { border-color: CanvasText; }
    .session-tile-dot { background: ButtonText; forced-color-adjust: none; }
    .session-tile-dot[data-state='empty'] { background: GrayText; }
    .session-tile-dot[data-state='attention'] { background: Highlight; }
  }
</style>
