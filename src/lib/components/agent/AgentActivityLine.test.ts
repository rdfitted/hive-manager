// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
vi.mock('$lib/stores/agentActivity', async () => {
  const { writable } = await import('svelte/store');
  return { agentActivity: writable({}), now: writable(Date.parse('2026-10-21T12:05:00Z')) };
});
import { agentActivity } from '$lib/stores/agentActivity';
import AgentActivityLine from './AgentActivityLine.svelte';
const set = (value: unknown) => (agentActivity as unknown as { set(v: unknown): void }).set(value);
afterEach(cleanup);
describe('AgentActivityLine', () => {
  it('handles a missing heartbeat without an invalid relative time', () => {
    set({}); const view = render(AgentActivityLine, { agentId: 'a' });
    expect(view.getByText('No activity yet')).toBeTruthy();
    expect(view.container.textContent).not.toMatch(/NaN|ago/);
  });
  it('falls back to the latest event for an empty summary', () => {
    set({ a: { summary: '', latestEventLabel: 'agent launched', lastActivityAt: Date.parse('2026-10-21T12:03:00Z') } });
    const view = render(AgentActivityLine, { agentId: 'a', compact: true });
    expect(view.getByText('agent launched')).toBeTruthy(); expect(view.getByText('· 2m ago')).toBeTruthy();
  });
  it('keeps a long summary on one truncated line with the full text in a tooltip', () => {
    const summary = 'Investigating a long task '.repeat(30);
    set({ a: { summary, lastActivityAt: null } });
    const view = render(AgentActivityLine, { agentId: 'a' });
    expect(view.container.querySelector('.activity-line')?.getAttribute('title')).toBe(summary);
    expect(view.container.querySelector('.summary')?.textContent).toBe(summary);
  });
});
