<script lang="ts">
  import { heartbeatStore } from '$lib/stores/conversations';
  import { needsAttention, workStateFor } from '$lib/stores/agentActivity';
  import { serdeEnumVariantName, sessionDisplayName, type Session } from '$lib/stores/sessions';

  let { sessions, onopen, limit = 3 }: {
    sessions: Session[];
    onopen: (id: string) => void;
    limit?: number;
  } = $props();

  const emptySeats = new Set<string>();
  const recent = $derived([...sessions]
    .sort((a, b) => Date.parse(b.last_activity_at ?? b.created_at) - Date.parse(a.last_activity_at ?? a.created_at))
    .slice(0, limit));

  function tone(session: Session, index: number) {
    const agent = session.agents[index];
    if (!agent) return 'empty';
    const stale = $heartbeatStore.staleAgentsBySession?.[session.id] ?? emptySeats;
    const heartbeat = $heartbeatStore.agentsBySession?.[session.id]?.[agent.id];
    if (needsAttention(agent, stale, workStateFor(agent, heartbeat))) return 'attention';
    return serdeEnumVariantName(agent.status)?.toLowerCase() ?? 'empty';
  }

  function summary(session: Session) {
    const stale = $heartbeatStore.staleAgentsBySession?.[session.id] ?? emptySeats;
    const waiting = session.agents.filter(agent => needsAttention(agent, stale,
      workStateFor(agent, $heartbeatStore.agentsBySession?.[session.id]?.[agent.id]))).length;
    const state = serdeEnumVariantName(session.state) ?? 'Unknown';
    return waiting ? `${state} · ${waiting} need you` : `${state} · ${session.agents.length} ${session.agents.length === 1 ? 'agent' : 'agents'}`;
  }
</script>

{#if recent.length}
  <section class="recent-hives" aria-labelledby="recent-hives-title">
    <h2 id="recent-hives-title">Open hives</h2>
    <ul>
      {#each recent as session (session.id)}
        <li>
          <button type="button" class="recent-card" onclick={() => onopen(session.id)}>
            <!-- A miniature of the stage: the Queen on the left, the first workers stacked beside her. -->
            <span class="thumb" aria-hidden="true">
              <i data-tone={tone(session, 0)}></i>
              <span>
                {#each [1, 2, 3] as index (index)}<i data-tone={tone(session, index)}></i>{/each}
              </span>
            </span>
            <span class="name">{sessionDisplayName(session)}</span>
            <small>{summary(session)}</small>
          </button>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .recent-hives { width: min(100%, 41.25rem); }
  h2 { margin: var(--space-2) 0 14px; color: var(--ink-muted); font: 500 17px/1.3 var(--font-display); text-align: center; }
  ul { display: grid; grid-template-columns: repeat(auto-fit, minmax(10rem, 13rem)); justify-content: center; gap: 18px; margin: 0; padding: 0; list-style: none; }
  .recent-card { display: grid; width: 100%; padding: 0; border: 0; border-radius: var(--control-radius); background: transparent; color: var(--ink); font: inherit; text-align: left; cursor: pointer; }
  .recent-card:focus-visible { outline: 2px solid var(--focus); outline-offset: 4px; }
  .thumb { display: grid; grid-template-columns: 1.4fr 1fr; gap: 5px; height: 112px; padding: 7px; border: 1px solid var(--line-strong); border-radius: var(--control-radius); background: var(--well); transition: border-color var(--motion-ui); }
  .recent-card:hover .thumb { border-color: var(--border-control); }
  .thumb > span { display: grid; gap: 5px; }
  /* Each cell is tinted by its agent's state, like the rail tiles; no edge stripes. */
  .thumb i { border-radius: 4px; background: color-mix(in srgb, var(--ink) 6%, transparent); }
  .thumb i[data-tone='running'] { background: color-mix(in srgb, var(--tone-info) 30%, transparent); }
  .thumb i[data-tone='attention'] { background: color-mix(in srgb, var(--accent) 46%, transparent); }
  .thumb i[data-tone='completed'] { background: color-mix(in srgb, var(--tone-ok) 22%, transparent); }
  .thumb i[data-tone='error'] { background: color-mix(in srgb, var(--tone-err) 34%, transparent); }
  .name { margin: 10px 0 2px; overflow: hidden; font: 500 16px/1.3 var(--font-display); text-overflow: ellipsis; white-space: nowrap; }
  small { color: var(--ink-muted); font-size: 11.5px; }
  @media (forced-colors: active) {
    .thumb { border-color: ButtonText; }
    .thumb i { background: GrayText; forced-color-adjust: none; }
    .thumb i[data-tone='attention'] { background: Highlight; }
  }
</style>
