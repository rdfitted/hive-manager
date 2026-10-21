import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushEnter } from './flush';

afterEach(() => vi.unstubAllGlobals());
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
