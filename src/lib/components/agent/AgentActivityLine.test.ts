// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import { tick } from 'svelte';
import source from './AgentActivityLine.svelte?raw';
vi.mock('$lib/stores/agentActivity', async () => {
  const { writable } = await import('svelte/store');
  return { agentActivity: writable({}), now: writable(Date.parse('2026-10-21T12:05:00Z')) };
});
import { agentActivity, now } from '$lib/stores/agentActivity';
import AgentActivityLine from './AgentActivityLine.svelte';
const set = (value: unknown) => (agentActivity as unknown as { set(v: unknown): void }).set(value);
afterEach(cleanup);
describe('AgentActivityLine', () => {
  it('updates relative time from the shared ticker without changing activity', async () => {
    (now as unknown as { set(v: number): void }).set(Date.parse('2026-10-21T12:05:00Z'));
    set({ a: { summary: 'Working', lastActivityAt: Date.parse('2026-10-21T12:03:00Z') } });
    const view = render(AgentActivityLine, { agentId: 'a' });
    expect(view.getByText('· 2m ago')).toBeTruthy();
    (now as unknown as { set(v: number): void }).set(Date.parse('2026-10-21T12:06:00Z'));
    await tick(); expect(view.getByText('· 3m ago')).toBeTruthy();
    (now as unknown as { set(v: number): void }).set(Date.parse('2026-10-21T12:05:00Z'));
  });
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
    expect(source).toMatch(/\.summary\s*\{[^}]*overflow: hidden;[^}]*text-overflow: ellipsis;[^}]*white-space: nowrap;/);
  });
});
