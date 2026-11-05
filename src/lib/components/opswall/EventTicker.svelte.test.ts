import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import EventTicker from './EventTicker.svelte';
import { tickerItems } from './ticker';
import type { AgentInfo } from '$lib/stores/sessions';
afterEach(cleanup);
it('G59 ticker merges session/coordination newest-first and selects the exact source column', async () => {
  const agents: AgentInfo[] = [{ id: 'alpha', role: 'Queen', parent_id: null, status: 'Running', config: { cli: 'codex', flags: [], label: 'Alpha' } }];
  const items = tickerItems('s', agents, [{ id: 'task', timestamp: '2030-01-01T12:00:02Z', from: 'operator', to: 'Alpha', message_type: 'Task', content: 'Inspect fixture' }], [
    { id: 'launch', session_id: 's', agent_id: 'alpha', timestamp: '2030-01-01T12:00:03Z', event_type: 'agent_launched', severity: 'info', payload: {} },
    { id: 'phase', session_id: 's', timestamp: '2030-01-01T12:00:01Z', event_type: 'session_status_changed', severity: 'info', payload: {} },
    { id: 'foreign', session_id: 'elsewhere', agent_id: 'alpha', timestamp: '2030-01-01T12:00:09Z', event_type: 'agent_failed', severity: 'error', payload: {} },
  ]);
  expect(items.map(item => item.id)).toEqual(['event:launch', 'coord:task', 'event:phase']);
  expect(items.map(item => item.agentId)).toEqual(['alpha', 'alpha', null]);
  const onselect = vi.fn();
  const view = render(EventTicker, { items, onselect });
  expect(view.getAllByRole('button').map(button => button.textContent)).toEqual(['agent launched', 'Task: Inspect fixture']);
  await fireEvent.click(view.getByRole('button', { name: 'Task: Inspect fixture' }));
  expect(onselect).toHaveBeenCalledTimes(1); expect(onselect).toHaveBeenCalledWith('alpha');
  expect(view.getByText('session status changed').tagName).toBe('SPAN');
});
