import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { writable } from 'svelte/store';
import type { Session } from '$lib/stores/sessions';

vi.mock('$lib/stores/sessions', async () => ({ activeSession: (await import('svelte/store')).writable(null) }));
import { activeSession } from '$lib/stores/sessions';
import ReviewView from './ReviewView.svelte';

const fetchMock = vi.fn();
const cells = [
  { id: 'cell-one', name: 'First', agents: ['worker-a', 'worker-b'] },
  { id: 'cell-two', name: 'Second', agents: ['worker-c', 'worker-b'] },
  { id: 'cell-three', name: 'Third', agents: ['worker-d'] },
];
beforeEach(() => {
  (activeSession as ReturnType<typeof writable<Session | null>>).set({ id: 'synthetic' } as Session);
  fetchMock.mockReset().mockImplementation(async (input, options) => {
    const url = new URL(String(input), 'http://localhost');
    if (options?.method === 'POST') return { ok: true, text: async () => 'synthetic receipt' };
    if (url.pathname.endsWith('/cells')) return { ok: true, json: async () => cells };
    return { ok: true, json: async () => ({ base: 'main', agent_id: url.searchParams.get('agent_id'),
      files: [{ path: 'example.ts', unified: '@@ -1 +1 @@\n-old\n+new', binary: false, truncated: false }], truncated: false }) };
  });
  vi.stubGlobal('fetch', fetchMock);
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe('G52 Review selection', () => {
  it('keeps the chosen principal, draft, reviewed mark, diff and receipt for a same-value Cell change', async () => {
    const view = render(ReviewView);
    const principal = view.getByLabelText('Principal') as HTMLSelectElement;
    await waitFor(() => expect(view.getByLabelText('File comment for example.ts')).toBeTruthy());
    await fireEvent.change(principal, { target: { value: 'worker-b' } });
    await waitFor(() => expect(view.getByLabelText('File comment for example.ts')).toBeTruthy());
    await fireEvent.input(view.getByLabelText('File comment for example.ts'), { target: { value: 'Keep this draft' } });
    await fireEvent.click(view.getByRole('checkbox', { name: 'Reviewed example.ts' }));
    await fireEvent.click(view.getByRole('button', { name: 'Send requested changes' }));
    await waitFor(() => expect(view.getByText('synthetic receipt')).toBeTruthy());
    const requests = fetchMock.mock.calls.length;
    await fireEvent.change(view.getByLabelText('Cell'), { target: { value: 'cell-one' } });
    expect(principal.value).toBe('worker-b');
    expect((view.getByLabelText('File comment for example.ts') as HTMLTextAreaElement).value).toBe('Keep this draft');
    expect((view.getByRole('checkbox', { name: 'Reviewed example.ts' }) as HTMLInputElement).checked).toBe(true);
    expect(view.getByText('Base: main')).toBeTruthy();
    expect(view.getByText('synthetic receipt')).toBeTruthy();
    expect(fetchMock).toHaveBeenCalledTimes(requests);
  });

  it('retains the principal when present in a new cell and otherwise picks its first principal', async () => {
    const view = render(ReviewView);
    const cell = view.getByLabelText('Cell');
    const principal = view.getByLabelText('Principal') as HTMLSelectElement;
    await waitFor(() => expect(view.getByLabelText('File comment for example.ts')).toBeTruthy());
    await fireEvent.change(principal, { target: { value: 'worker-b' } });
    await fireEvent.change(cell, { target: { value: 'cell-two' } });
    await waitFor(() => expect(principal.value).toBe('worker-b'));
    expect(String(fetchMock.mock.calls.at(-1)?.[0])).toContain('/cells/cell-two/review-diff?agent_id=worker-b');
    await fireEvent.change(cell, { target: { value: 'cell-three' } });
    await waitFor(() => expect(principal.value).toBe('worker-d'));
    expect(String(fetchMock.mock.calls.at(-1)?.[0])).toContain('/cells/cell-three/review-diff?agent_id=worker-d');
  });
});
