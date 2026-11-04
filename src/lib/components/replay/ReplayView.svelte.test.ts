import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import { writable } from 'svelte/store';
import type { Event } from '$lib/types/domain';

vi.mock('$lib/stores/replay', async () => ({ eventsAtTimestamp: (await import('svelte/store')).writable([]) }));
vi.mock('$lib/stores/sessions', async () => ({
  activeSession: (await import('svelte/store')).writable(null),
  serdeEnumVariantName: (value: unknown) => typeof value === 'string' ? value : undefined,
}));
import { eventsAtTimestamp } from '$lib/stores/replay';
import ReplayView from './ReplayView.svelte';

afterEach(cleanup);
describe('G52 replay stop outcome', () => {
  it.each([true, false, undefined])('uses preserved=%s even after a previous completion', (preserved) => {
    const event = (event_type: Event['event_type'], payload = {}): Event => ({
      id: event_type, session_id: 's', agent_id: 'worker-a', event_type,
      timestamp: '2026-11-01T12:00:00Z', severity: 'info', payload,
    });
    (eventsAtTimestamp as ReturnType<typeof writable<Event[]>>).set([
      event('agent_launched'), event('agent_completed'),
      event('agent_stopped', preserved === undefined ? {} : { preserved }),
    ]);
    const view = render(ReplayView);
    const badge = view.getByText(preserved === true ? 'completed' : 'killed');
    expect(badge.classList.contains(preserved === true ? 'status-success' : 'status-canceled')).toBe(true);
  });
});
