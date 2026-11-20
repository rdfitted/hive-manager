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
    <div class="activity-lanes lattice-scroll-content" role="group" aria-label="Agent activity">
      {#each lanes as agent (agent.id)}
        {@const latest = agent.marks[0]}
        <button type="button" class="activity-lane lattice-btn lattice-btn--ghost"
          onclick={() => onselect(agent.id)} title={latest?.text} aria-label={`Reveal ${agent.label}`}>
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
  <ResourceStrip attention={attention.size} {waiting} stale={stale.size} {phase} {filter} {onfilter}
    metrics={band ? { agents: session.agents.length, running, elapsed, nodes, files } : undefined} />
  <EventTicker {items} {onselect} />
</div>

<style>
  .ops-chrome { flex: 0 0 auto; min-width: 0; padding: 0 var(--space-1) var(--space-1); }
  .ops-chrome.band { padding: var(--space-2) var(--space-3); background: var(--chrome); border-top: 1px solid var(--line); }
  .activity-lanes { display: grid; max-height: 6rem; overflow: auto; }
  .activity-lane { display: grid; grid-template-columns: minmax(4rem, 9rem) minmax(0, 1fr); gap: var(--space-2); justify-items: start; min-height: 1.5rem; padding: 0 var(--space-2); }
  .activity-lane strong { max-width: 100%; overflow: hidden; text-overflow: ellipsis; font-size: var(--text-micro); }
  .activity-track { position: relative; width: 100%; height: var(--space-2); background: var(--surface-raised); border: 1px solid var(--line); border-radius: var(--radius-sm); }
  .activity-mark { position: absolute; top: 0; bottom: 0; width: 2px; background: var(--status-running); }
  .activity-track.hot .activity-mark { background: var(--accent); }
  @media (forced-colors: active) {
    .ops-chrome.band, .activity-track { border-color: CanvasText; }
    .activity-mark, .activity-track.hot .activity-mark { background: Highlight; forced-color-adjust: none; }
  }
</style>
