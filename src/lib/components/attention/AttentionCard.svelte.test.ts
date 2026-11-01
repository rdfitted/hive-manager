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
import FlushReceipt from './FlushReceipt.svelte';

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
  it('names a startup death without offering to flush or resend into a dead process', () => {
    const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'exited_during_startup:101' }) });
    expect(view.getByRole('status').textContent).toBe("This agent's process exited during startup with code 101. It has not been restarted.");
    expect(view.queryByRole('button', { name: 'Flush Enter' })).toBeNull();
    expect(view.queryByRole('button', { name: 'Resend' })).toBeNull();
    expect(view.getByRole('button', { name: 'Open terminal' })).toBeTruthy();
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
  it('flushes once and displays a sentence with the exact receipt in closed Details', async () => {
    const receipt = '{"submit_confirmed":null,"submit_bytes_written":1}';
    const fetcher = vi.fn().mockResolvedValue({ ok: true, text: async () => receipt });
    vi.stubGlobal('fetch', fetcher);
    const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
    await fireEvent.click(view.getByRole('button', { name: 'Flush Enter' }));
    await waitFor(() => expect(view.getByRole('status').textContent).toBe("Enter sent. Delivery isn't confirmed yet."));
    const details = view.container.querySelector('details')!;
    expect(details.open).toBe(false);
    expect(details.querySelector('pre')?.textContent).toBe(receipt);
    expect(JSON.parse(fetcher.mock.calls[0][1].body)).toEqual({ target_agent_id: 'worker-5', message: '', submit: true });
    expect(fetcher).toHaveBeenCalledOnce();
  });
  it.each([
    ['{"submit_confirmed":true}', 'Enter delivered.'],
    ['{"submit_confirmed":false}', 'Enter may not have landed. Check the terminal.'],
    ['legacy receipt', 'Enter sent.'],
    ['', 'Enter sent.'],
  ])('G21: summarizes %s and dismisses both sentence and raw receipt', async (receipt, sentence) => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, text: async () => receipt }));
    const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
    const flush = view.getByRole('button', { name: 'Flush Enter' }); flush.focus();
    await fireEvent.click(flush);
    await waitFor(() => expect(view.getByRole('status').textContent, 'receipt status must be a plain sentence').toBe(sentence));
    expect(document.activeElement).toBe(flush);
    expect(view.container.querySelector('details')?.open).toBe(false);
    expect(view.container.querySelector('pre')?.textContent).toBe(receipt);
    await fireEvent.click(view.getByRole('button', { name: 'Dismiss Enter receipt' }));
    expect(view.queryByRole('status')).toBeNull();
    expect(view.container.querySelector('details')).toBeNull();
  });
  it('shows a failed flush without resending', async () => {
    const fetcher = vi.fn().mockRejectedValue(new Error('offline'));
    vi.stubGlobal('fetch', fetcher);
    const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
    await fireEvent.click(view.getByRole('button', { name: 'Flush Enter' }));
    await waitFor(() => expect(view.getByRole('alert').textContent).toContain('offline'));
    expect(fetcher).toHaveBeenCalledOnce();
  });
  it('G21: keeps a failed flush visible after the receipt clear interval', async () => {
    vi.useFakeTimers();
    try {
      vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline fixture')));
      const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
      await fireEvent.click(view.getByRole('button', { name: 'Flush Enter' }));
      await vi.advanceTimersByTimeAsync(60000);
      expect(view.getByRole('alert').textContent).toContain('offline fixture');
    } finally { vi.useRealTimers(); }
  });
  it('G21: removes successful sentence and Details at eight seconds', async () => {
    vi.useFakeTimers();
    try {
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, text: async () => '{"submit_confirmed":true}' }));
      const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
      await fireEvent.click(view.getByRole('button', { name: 'Flush Enter' }));
      await vi.advanceTimersByTimeAsync(0);
      expect(view.getByRole('status').textContent).toBe('Enter delivered.');
      await vi.advanceTimersByTimeAsync(7999);
      expect(view.getByRole('status')).toBeTruthy();
      await vi.advanceTimersByTimeAsync(1);
      expect(view.queryByRole('status')).toBeNull();
      expect(view.container.querySelector('details')).toBeNull();
    } finally { vi.useRealTimers(); }
  });
  it.each([true, null])('F2: pauses the remaining receipt lifetime while Details is open (%s)', async (confirmed) => {
    vi.useFakeTimers();
    try {
      const receipt = JSON.stringify({ submit_confirmed: confirmed });
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, text: async () => receipt }));
      const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
      const flush = view.getByRole('button', { name: 'Flush Enter' }); flush.focus();
      await fireEvent.click(flush);
      await vi.advanceTimersByTimeAsync(0);
      const sentence = view.getByRole('status').textContent;
      await vi.advanceTimersByTimeAsync(7900);
      const details = view.container.querySelector('details')!;
      details.open = true; await fireEvent(details, new Event('toggle'));
      await vi.advanceTimersByTimeAsync(60000);
      expect(view.getByRole('status').textContent, 'open receipt survives its clear interval').toBe(sentence);
      expect(details.isConnected).toBe(true);
      expect(details.open).toBe(true);
      expect(details.querySelector('pre')?.textContent).toBe(receipt);
      expect(document.activeElement).toBe(flush);
      details.open = false; await fireEvent(details, new Event('toggle'));
      await vi.advanceTimersByTimeAsync(99);
      expect(view.getByRole('status')).toBeTruthy();
      await vi.advanceTimersByTimeAsync(1);
      expect(view.queryByRole('status'), 'closing resumes only the remaining 100ms').toBeNull();
      expect(view.container.querySelector('details')).toBeNull();
      expect(document.activeElement).toBe(flush);
    } finally { vi.useRealTimers(); }
  });
  it('F2: false confirmation persists with Details closed or open', async () => {
    vi.useFakeTimers();
    try {
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, text: async () => '{"submit_confirmed":false}' }));
      const view = render(AttentionCard, { sessionId: 's', agent: agent({ Error: 'failed' }) });
      await fireEvent.click(view.getByRole('button', { name: 'Flush Enter' }));
      await vi.advanceTimersByTimeAsync(60000);
      expect(view.getByRole('status').textContent).toContain('Enter may not have landed.');
      const details = view.container.querySelector('details')!;
      details.open = true; await fireEvent(details, new Event('toggle'));
      await vi.advanceTimersByTimeAsync(60000);
      await fireEvent.click(view.getByRole('button', { name: 'Dismiss Enter receipt' }));
      expect(view.queryByRole('status')).toBeNull();
    } finally { vi.useRealTimers(); }
  });
  it('F2: a replacement receipt closes Details and starts a fresh eight seconds', async () => {
    vi.useFakeTimers();
    try {
      const ondismiss = vi.fn();
      const view = render(FlushReceipt, { body: '{"submit_confirmed":true,"id":1}', ondismiss });
      await vi.advanceTimersByTimeAsync(7900);
      const details = view.container.querySelector('details')!;
      details.open = true; await fireEvent(details, new Event('toggle'));
      await vi.advanceTimersByTimeAsync(60000);
      expect(ondismiss).not.toHaveBeenCalled();
      await view.rerender({ body: '{"submit_confirmed":null,"id":2}' });
      expect(view.container.querySelector('details')?.open).toBe(false);
      await vi.advanceTimersByTimeAsync(7999);
      expect(ondismiss).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      expect(ondismiss).toHaveBeenCalledOnce();
    } finally { vi.useRealTimers(); }
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
