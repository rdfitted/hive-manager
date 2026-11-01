// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushEnter } from './flush';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import { tick } from 'svelte';
import FlushReceipt, { receiptSummary } from './FlushReceipt.svelte';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.useRealTimers(); });
describe('flushEnter', () => {
  it('posts only a bare Enter and returns the unmodified HTTP receipt', async () => {
    const receipt = '{"submit_confirmed":null,"bytes_written":1}';
    const fetcher = vi.fn().mockResolvedValue({ ok: true, text: async () => receipt });
    vi.stubGlobal('fetch', fetcher);
    expect(await flushEnter('session/a', 'worker-5')).toBe(receipt);
    expect(fetcher).toHaveBeenCalledOnce();
    expect(fetcher).toHaveBeenCalledWith(expect.stringContaining('/api/sessions/session%2Fa/inject'), {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ target_agent_id: 'worker-5', message: '', submit: true }),
    });
  });
  it('surfaces HTTP errors without retrying an uncertain delivery', async () => {
    const fetcher = vi.fn().mockResolvedValue({ ok: false, status: 409, text: async () => 'delivery pending' });
    vi.stubGlobal('fetch', fetcher);
    await expect(flushEnter('s', 'a')).rejects.toThrow('Flush failed (409): delivery pending');
    expect(fetcher).toHaveBeenCalledOnce();
  });
});

describe('G21 shared Flush Enter receipt', () => {
  it.each([
    ['{"submit_confirmed":true}', 'Enter delivered.', false],
    ['{"submit_confirmed":null}', "Enter sent. Delivery isn't confirmed yet.", false],
    ['{"submit_confirmed":false}', 'Enter may not have landed. Check the terminal.', true],
    ['non-JSON fixture', 'Enter sent.', false],
    ['null', 'Enter sent.', false],
  ])('summarizes %s with the correct persistence', (body, sentence, persistent) => {
    expect(receiptSummary(body as string)).toEqual({ sentence, persistent });
  });

  it.each(['{"submit_confirmed":true}', '{"submit_confirmed":null}', 'non-JSON fixture'])('clears %s at eight seconds', async (body) => {
    vi.useFakeTimers();
    const dismiss = vi.fn();
    render(FlushReceipt, { body, ondismiss: dismiss });
    await vi.advanceTimersByTimeAsync(7999);
    expect(dismiss).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(dismiss).toHaveBeenCalledOnce();
  });

  it('keeps false visible until dismissed, with Details closed by default', async () => {
    vi.useFakeTimers();
    const dismiss = vi.fn();
    const view = render(FlushReceipt, { body: '{"submit_confirmed":false}', ondismiss: dismiss });
    await vi.advanceTimersByTimeAsync(60000);
    expect(dismiss).not.toHaveBeenCalled();
    expect(view.getByRole('status').textContent).toBe('Enter may not have landed. Check the terminal.');
    expect(view.container.querySelector('details')?.open).toBe(false);
    await fireEvent.click(view.getByRole('button', { name: 'Dismiss Enter receipt' }));
    expect(dismiss).toHaveBeenCalledOnce();
  });

  it('cancels the old clear when a persistent receipt replaces a successful receipt', async () => {
    vi.useFakeTimers();
    const dismiss = vi.fn();
    const view = render(FlushReceipt, { body: '{"submit_confirmed":true}', ondismiss: dismiss });
    await vi.advanceTimersByTimeAsync(4000);
    await view.rerender({ body: '{"submit_confirmed":false}', ondismiss: dismiss }); await tick();
    await vi.advanceTimersByTimeAsync(60000);
    expect(dismiss).not.toHaveBeenCalled();
  });
});
