// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { Writable } from 'svelte/store';

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return { activeSession: writable<{ id: string } | null>({ id: 'session-a' }) };
});
import { activeSession } from '$lib/stores/sessions';
import ReviewView from './ReviewView.svelte';

const cells = [{ id: 'primary', name: 'Primary', agents: ['worker-a', 'worker-b'] }];
const fixture = (agent = 'worker-a') => ({
  base: 'recorded-base', agent_id: agent, truncated: false,
  files: [
    { path: 'one.ts', unified: '@@ -1 +1 @@\n-oldOne\n+newOne', binary: false, truncated: false },
    { path: 'two.ts', unified: '@@ -1 +1 @@\n-oldTwo\n+newTwo', binary: false, truncated: false },
    { path: 'image.png', unified: '', binary: true, truncated: true },
  ],
});
const response = (value: unknown) => ({ ok: true, json: async () => value, text: async () => JSON.stringify(value) }) as Response;
const setSession = (value: { id: string } | null) => (activeSession as unknown as Writable<{ id: string } | null>).set(value);

beforeEach(() => { setSession({ id: 'session-a' }); });
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe('operator review', () => {
  it('renders each text file through the existing Diff renderer and lists binary files', async () => {
    vi.stubGlobal('fetch', vi.fn(async (url: string) => response(url.endsWith('/cells') ? cells : fixture())));
    const view = render(ReviewView);
    await view.findByText('newTwo');
    expect(view.container.querySelectorAll('.diff-widget')).toHaveLength(2);
    expect(view.container.querySelectorAll('.diff-line.add')).toHaveLength(2);
    expect(view.getByText('newOne')).toBeTruthy();
    expect(view.getByText('Binary file — preview unavailable.')).toBeTruthy();
    await fireEvent.click(view.getByLabelText('Reviewed one.ts'));
    expect((view.getByLabelText('Reviewed one.ts') as HTMLInputElement).checked).toBe(true);
  });

  it('combines file and line comments in exactly one inject and displays the raw receipt', async () => {
    const rawReceipt = '{ "submit_confirmed": false, "basis": "transcript", "extra": [1] }';
    let finish!: (response: Response) => void;
    const fetchMock = vi.fn((url: string, init?: RequestInit) => {
      if (init?.method === 'POST') return new Promise<Response>((resolve) => { finish = resolve; });
      return Promise.resolve(response(url.endsWith('/cells') ? cells : fixture()));
    });
    vi.stubGlobal('fetch', fetchMock);
    const view = render(ReviewView);
    await view.findByText('newTwo');
    await fireEvent.input(view.getByLabelText('File comment for one.ts'), { target: { value: 'Handle failure' } });
    await fireEvent.input(view.getByLabelText('Line number for two.ts'), { target: { value: '8' } });
    await fireEvent.input(view.getByLabelText('Line comment for two.ts'), { target: { value: 'Keep this guard' } });
    const button = view.getByRole('button', { name: 'Send requested changes' });
    await fireEvent.click(button);
    await fireEvent.click(button);
    const posts = fetchMock.mock.calls.filter((call) => call[1]?.method === 'POST');
    expect(posts).toHaveLength(1);
    expect(posts[0][0]).toContain('/api/sessions/session-a/inject');
    const body = JSON.parse(posts[0][1]!.body as string);
    expect(body.target_agent_id).toBe('worker-a');
    expect(body.message).toContain('one.ts: Handle failure');
    expect(body.message).toContain('two.ts:8: Keep this guard');
    finish({ ok: true, text: async () => rawReceipt } as Response);
    await waitFor(() => expect(view.getByRole('region', { name: 'Injection receipt' }).querySelector('pre')?.textContent).toBe(rawReceipt));
  });

  it('ignores a stale diff after a principal switch', async () => {
    let finish!: (value: Response) => void;
    vi.stubGlobal('fetch', vi.fn((url: string) => {
      if (url.endsWith('/cells')) return Promise.resolve(response(cells));
      if (url.includes('worker-a')) return new Promise<Response>((resolve) => { finish = resolve; });
      return Promise.resolve(response({ ...fixture('worker-b'), files: [] }));
    }));
    const view = render(ReviewView);
    await waitFor(() => expect(finish).toBeTypeOf('function'));
    await fireEvent.change(view.getByLabelText('Principal'), { target: { value: 'worker-b' } });
    await view.findByText('No changes for this principal.');
    const stale = response(fixture());
    const parsed = vi.spyOn(stale, 'json');
    finish(stale);
    await waitFor(() => expect(parsed).toHaveBeenCalled());
    await tick();
    expect(view.queryByText('newOne')).toBeNull();
    expect(view.getByText('No changes for this principal.')).toBeTruthy();
  });
});
