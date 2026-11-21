<script lang="ts">
  import type { Session } from '$lib/stores/sessions';
  import { serdeEnumVariantName } from '$lib/stores/sessions';
  import { coordination } from '$lib/stores/coordination';
  import { events } from '$lib/stores/events';
  import ResourceStrip from './ResourceStrip.svelte';
  import EventTicker from './EventTicker.svelte';
  import { activityLanes, bandFileCount, bandNodeCount, tickerItems } from './ticker';
  import { formatElapsedTime } from '$lib/workgraph/graphUtils';
  import { TERMINAL_SESSION_STATES } from '$lib/stores/terminalPool';

  let { band = false, session, attention, stale, filter, onfilter, onselect }: {
    band?: boolean; session: Session; attention: ReadonlySet<string>; stale: ReadonlySet<string>; filter: string | null;
    onfilter: (filter: string | null) => void; onselect: (id: string) => void;
  } = $props();
  const waiting = $derived(session.agents.filter(agent => serdeEnumVariantName(agent.status) === 'WaitingForInput').length);
  const items = $derived(tickerItems(session.id, session.agents, $coordination.sessionId === session.id ? $coordination.log ?? [] : [], $events.events));
  const phase = $derived(serdeEnumVariantName(session.state) ?? 'Unknown');
  const running = $derived(session.agents.filter(agent => serdeEnumVariantName(agent.status) === 'Running').length);
  const lanes = $derived(band ? activityLanes(session.agents, items) : []);
  let now = $state(Date.now());
  let nodes = $state<string | null>(null);
  let files = $state<number | null>(null);
  const elapsed = $derived(formatElapsedTime(
    session.created_at,
    TERMINAL_SESSION_STATES.has(phase) ? session.last_activity_at ?? session.created_at : null,
    now,
  ));

  $effect(() => {
    if (!band) return;
    const id = session.id;
    const controller = new AbortController();
    const { signal } = controller;
    let refreshGeneration = 0;
    nodes = null;
    files = null;

    async function refresh() {
      const generation = ++refreshGeneration;
      const [nodeResult, fileResult] = await Promise.allSettled([
        bandNodeCount(id, signal), bandFileCount(id, signal),
      ]);
      if (signal.aborted || generation !== refreshGeneration) return;
      if (nodeResult.status === 'fulfilled') nodes = nodeResult.value;
      if (fileResult.status === 'fulfilled') files = fileResult.value;
    }

    void refresh();
    const interval = setInterval(() => {
      now = Date.now();
      void refresh();
    }, 30_000);
    return () => {
      controller.abort();
      clearInterval(interval);
    };
  });

  $effect(() => {
    const id = session.id;
    coordination.setSessionId(id);
    void coordination.ensureLog(id);
  });
</script>

<div class="ops-chrome" class:band>
  {#if lanes.length}
    <div class="activity-lanes" role="group" aria-label="Agent activity" style:--lanes={lanes.length}>
      {#each lanes as agent (agent.id)}
        {@const latest = agent.marks[0]}
        <button type="button" class="activity-lane lattice-btn lattice-btn--ghost"
          onclick={() => onselect(agent.id)} title={latest ? `${agent.label} · ${latest.text}` : agent.label} aria-label={`Reveal ${agent.label}`}>
          <strong>{agent.label}</strong>
          <span class="activity-track" class:hot={attention.has(agent.id)} aria-hidden="true">
            {#each agent.marks as mark (mark.id)}
              <span class="activity-mark" style:left={`${mark.position}%`}></span>
            {/each}
          </span>
        </button>
      {/each}
    </div>
  {/if}
  {#if band}
    <ResourceStrip attention={attention.size} {waiting} stale={stale.size} {phase} {filter} {onfilter}
      metrics={{ agents: session.agents.length, running, elapsed, nodes, files }}>
      <EventTicker {items} {onselect} />
    </ResourceStrip>
  {:else}
    <ResourceStrip attention={attention.size} {waiting} stale={stale.size} {phase} {filter} {onfilter} />
    <EventTicker {items} {onselect} />
  {/if}
</div>

<style>
  .ops-chrome { flex: 0 0 auto; min-width: 0; padding: 0 var(--space-1) var(--space-1); }
  /* Glass band under the stage. It never sits over a live terminal. */
  .ops-chrome.band {
    display: grid;
    gap: 6px;
    padding: 9px 14px 8px;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-glass);
    box-shadow: 0 10px 30px rgb(0 0 0 / .35);
  }
  @supports ((backdrop-filter: blur(1px)) or (-webkit-backdrop-filter: blur(1px))) {
    .ops-chrome.band {
      background: var(--glass);
      -webkit-backdrop-filter: var(--glass-blur);
      backdrop-filter: var(--glass-blur);
    }
  }
  /* One thin lane per agent; the bright rule on the right is now. */
  .activity-lanes {
    position: relative;
    display: grid;
    grid-auto-rows: minmax(2px, 1fr);
    gap: 2px;
    height: clamp(24px, calc(var(--lanes, 4) * 6px), 42px);
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: 6px;
  }
  .activity-lanes::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 98.4%;
    width: 2px;
    background: var(--ink);
    pointer-events: none;
  }
  .ops-chrome .activity-lane {
    position: relative;
    display: block;
    width: 100%;
    min-height: 0;
    padding: 0;
    border-radius: 0;
    background: rgb(255 255 255 / .025);
  }
  .ops-chrome .activity-lane:hover { background: var(--wash-strong); }
  .ops-chrome .activity-lane:focus-visible { outline-offset: -2px; }
  .activity-lane strong { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .activity-track { position: absolute; inset: 0; }
  .activity-mark { position: absolute; top: 0; bottom: 0; width: 1.4%; min-width: 4px; border-radius: 1px; background: var(--tone-info); opacity: .85; }
  .activity-track.hot .activity-mark { background: var(--accent); opacity: 1; }
  @media (forced-colors: active) {
    .ops-chrome.band { border-color: CanvasText; background: Canvas; -webkit-backdrop-filter: none; backdrop-filter: none; box-shadow: none; }
    .activity-lanes { border-color: CanvasText; }
    .activity-lanes::after { background: CanvasText; }
    .activity-mark, .activity-track.hot .activity-mark { background: Highlight; forced-color-adjust: none; opacity: 1; }
  }
</style>
