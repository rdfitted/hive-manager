<script lang="ts">
  import type { Session } from '$lib/stores/sessions';
  import { serdeEnumVariantName } from '$lib/stores/sessions';
  import { coordination } from '$lib/stores/coordination';
  import { events } from '$lib/stores/events';
  import ResourceStrip from './ResourceStrip.svelte';
  import EventTicker from './EventTicker.svelte';
  import { tickerItems } from './ticker';
  let { session, attention, stale, filter, onfilter, onselect }: {
    session: Session; attention: Set<string>; stale: Set<string>; filter: string | null;
    onfilter: (filter: string | null) => void; onselect: (id: string) => void;
  } = $props();
  const waiting = $derived(session.agents.filter(agent => serdeEnumVariantName(agent.status) === 'WaitingForInput').length);
  const items = $derived(tickerItems(session.id, session.agents, $coordination.sessionId === session.id ? $coordination.log ?? [] : [], $events.events));
  $effect(() => { const id = session.id; coordination.setSessionId(id); void coordination.loadLog(id, 30); });
</script>

<div class="ops-chrome">
  <ResourceStrip attention={attention.size} {waiting} stale={stale.size} phase={serdeEnumVariantName(session.state) ?? 'Unknown'} {filter} {onfilter} />
  <EventTicker {items} {onselect} />
</div>

<style>
  .ops-chrome { flex: 0 0 auto; min-width: 0; padding: 0 var(--space-1) var(--space-1); }
</style>
