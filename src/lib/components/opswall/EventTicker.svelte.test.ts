import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import EventTicker from './EventTicker.svelte';
import { tickerItems } from './ticker';
import type { AgentInfo } from '$lib/stores/sessions';
afterEach(cleanup);
it('G59 ticker resolves backend wire aliases independently of custom labels and never guesses duplicate aliases', () => {
  const agents: AgentInfo[] = [
    { id: 's-queen', role: 'Queen', parent_id: null, status: 'Running', config: { cli: 'codex', flags: [], label: 'Custom coordinator' } },
    { id: 's-worker-1', role: { Worker: { index: 1, parent: null } }, parent_id: 's-queen', status: 'Running', config: { cli: 'codex', flags: [], label: 'Custom worker' } },
    { id: 's-qa-worker-2', role: { QaWorker: { index: 2, parent: null } }, parent_id: 's-queen', status: 'Running', config: { cli: 'codex', flags: [], label: 'Custom QA' } },
    { id: 's-planner-3-round', role: { Planner: { index: 3 } }, parent_id: null, status: 'Running', config: { cli: 'codex', flags: [], label: 'Custom planner' } },
    { id: 's-evaluator', role: 'Evaluator', parent_id: null, status: 'Running', config: { cli: 'codex', flags: [], label: 'Custom evaluator' } },
  ];
  const aliases = ['QUEEN', 'WORKER-1', 'QA-WORKER-2', 'PLANNER-3', 'EVALUATOR'];
  const log = aliases.map((to, index) => ({ id: `wire-${index}`, timestamp: '2030-01-01T12:00:00Z', from: 'SYSTEM', to, message_type: 'Task' as const, content: 'Wire fixture' }));
  expect(tickerItems('s', agents, log, []).map(item => item.agentId)).toEqual(agents.map(agent => agent.id));
  const duplicates = [...agents, { ...agents[1], id: 's-other-worker-1' }];
  expect(tickerItems('s', duplicates, [log[1]], [])[0].agentId).toBeNull();
});
it('G59 ticker orders mixed-precision timestamps chronologically within one second', () => {
  const items = tickerItems('s', [], [{ id: 'coarse', timestamp: '2030-01-01T12:00:00Z', from: 'operator', to: 'all', message_type: 'Task', content: 'Coarse timestamp' }], [
    { id: 'fraction', session_id: 's', timestamp: '2030-01-01T12:00:00.900Z', event_type: 'agent_launched', severity: 'info', payload: {} },
  ]);
  expect(items.map(item => item.id)).toEqual(['event:fraction', 'coord:coarse']);
});
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
  expect(view.getByText('Latest events')).toBeTruthy();
  expect(view.getAllByRole('button').map(button => button.textContent)).toEqual(['agent launched', 'Task: Inspect fixture']);
  await fireEvent.click(view.getByRole('button', { name: 'Task: Inspect fixture' }));
  expect(onselect).toHaveBeenCalledTimes(1); expect(onselect).toHaveBeenCalledWith('alpha');
  expect(view.getByText('session status changed').tagName).toBe('SPAN');
});
