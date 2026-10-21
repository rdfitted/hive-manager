import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import type { AgentInfo } from '$lib/stores/sessions';

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return { activeAgents: writable([]) };
});
vi.mock('$lib/stores/events', async () => {
  const { writable } = await import('svelte/store');
  return { events: writable({ events: [] }) };
});
vi.mock('$lib/stores/conversations', async () => {
  const { writable } = await import('svelte/store');
  return { heartbeatStore: writable({ agents: {}, staleAgents: new Set<string>(), stalledAgents: new Set<string>() }) };
});
vi.mock('$lib/stores/layout', () => ({ layout: { setSessionViewMode: vi.fn() } }));
vi.mock('$lib/stores/ui', () => ({ ui: { setFocusedAgent: vi.fn() } }));
import { heartbeatStore } from '$lib/stores/conversations';
import { layout } from '$lib/stores/layout';
import { ui } from '$lib/stores/ui';
import AttentionCard from './AttentionCard.svelte';

const agent = (status: AgentInfo['status']): AgentInfo => ({ id: 'worker-5', role: 'Queen', config: { cli: 'codex', flags: [] }, parent_id: null, status });
beforeEach(() => {
  // The production store intentionally exposes mutations through events only.
  (heartbeatStore as unknown as { set: (value: unknown) => void }).set({ agents: {}, staleAgents: new Set(), stalledAgents: new Set() });
});
afterEach(() => { cleanup(); vi.clearAllMocks(); vi.unstubAllGlobals(); });

describe('AttentionCard', () => {
  it.each([
    [{ WaitingForInput: 'injected_delivery_failed:transcript:123' }, 'The last message could not be delivered.'],
    [{ Error: 'failed' }, 'This agent reported an error.'],
  ] as const)('explains %o using the shared attention predicate', (status, text) => {
    const view = render(AttentionCard, { sessionId: 's', agent: agent(status) });
    expect(view.getByText(text)).toBeTruthy();
    expect((view.getByRole('button', { name: 'Resend' }) as HTMLButtonElement).disabled).toBe(true);
    expect(view.getByText('original message unavailable')).toBeTruthy();
  });
  it('explains an independently stale heartbeat', () => {
    (heartbeatStore as unknown as { set: (value: unknown) => void }).set({ agents: {}, staleAgents: new Set(['worker-5']), stalledAgents: new Set() });
    expect(render(AttentionCard, { sessionId: 's', agent: agent('Running') }).getByText('No heartbeat has arrived for more than three minutes.')).toBeTruthy();
  });
  it.each(['awaiting_injected_turn:123', 'awaiting_injected_turn'])('renders no card for %s', (reason) => {
    const view = render(AttentionCard, { sessionId: 's', agent: agent({ WaitingForInput: reason }) });
    expect(view.queryByRole('region')).toBeNull();
    expect(view.container.textContent).toBe('');
  });
  it('flushes once and displays the exact receipt', async () => {
    const receipt = '{"submit_confirmed":null,"submit_bytes_written":1}';
    const fetcher = vi.fn().mockResolvedValue({ ok: true, text: async () => receipt });
    vi.stubGlobal('fetch', fetcher);
    const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
    await fireEvent.click(view.getByRole('button', { name: 'Flush Enter' }));
    await waitFor(() => expect(view.getByRole('status').textContent).toBe(receipt));
    expect(JSON.parse(fetcher.mock.calls[0][1].body)).toEqual({ target_agent_id: 'worker-5', message: '', submit: true });
    expect(fetcher).toHaveBeenCalledOnce();
  });
  it('shows a failed flush without resending', async () => {
    const fetcher = vi.fn().mockRejectedValue(new Error('offline'));
    vi.stubGlobal('fetch', fetcher);
    const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
    await fireEvent.click(view.getByRole('button', { name: 'Flush Enter' }));
    await waitFor(() => expect(view.getByRole('alert').textContent).toContain('offline'));
    expect(fetcher).toHaveBeenCalledOnce();
  });
  it('opens Focus mode and focuses the same agent', async () => {
    const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
    await fireEvent.click(view.getByRole('button', { name: 'Open terminal' }));
    expect(layout.setSessionViewMode).toHaveBeenCalledWith('focus');
    expect(ui.setFocusedAgent).toHaveBeenCalledWith('worker-5');
  });
  it('does not display another agent’s receipt after focus switches during a request', async () => {
    let resolve!: (response: unknown) => void;
    vi.stubGlobal('fetch', vi.fn(() => new Promise((done) => { resolve = done; })));
    const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
    await fireEvent.click(view.getByRole('button', { name: 'Flush Enter' }));
    await view.rerender({ sessionId: 's', agent: { ...agent({ Error: 'failed' }), id: 'other' } });
    resolve({ ok: true, text: async () => 'old receipt' });
    await waitFor(() => expect((view.getByRole('button', { name: 'Flush Enter' }) as HTMLButtonElement).disabled).toBe(false));
    expect(view.queryByText('old receipt')).toBeNull();
  });
});
