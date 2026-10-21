// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { ui } from '$lib/stores/ui';

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return { activeSession: writable({ id: 'session-a' }) };
});
const mounts = vi.hoisted(() => ({ files: vi.fn(), artifacts: vi.fn() }));
vi.mock('../SessionFilesView.svelte', () => ({ default: mounts.files }));
vi.mock('../artifacts/ArtifactBrowser.svelte', () => ({ default: mounts.artifacts }));
import ChangesTab from './ChangesTab.svelte';

const cells = [
  { id: 'first', name: 'First', agents: ['worker-a', 'worker-c'] },
  { id: 'focused-cell', name: 'Focused cell', agents: ['worker-b'] },
];
const response = (value: unknown) => ({ ok: true, json: async () => value, text: async () => JSON.stringify(value) }) as Response;
let fetchMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  ui.setFocusedAgent('worker-b');
  fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === 'POST') return response({ submit_confirmed: true, basis: 'transcript' });
    if (url.endsWith('/cells')) return response(cells);
    const agent = new URL(url, 'http://localhost').searchParams.get('agent_id');
    return response({ base: 'recorded-base', agent_id: agent, truncated: false, files: [
      { path: 'change.ts', unified: '@@ -1 +1 @@\n-old\n+new', binary: false, truncated: false },
    ] });
  });
  vi.stubGlobal('fetch', fetchMock);
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.clearAllMocks(); ui.setFocusedAgent(null); });

describe('Changes rail composition', () => {
  it('seeds the focused cell and principal, then sends review comments to that principal', async () => {
    const view = render(ChangesTab);
    await view.findByText('new');
    expect((view.getByLabelText('Cell') as HTMLSelectElement).value).toBe('focused-cell');
    expect((view.getByLabelText('Principal') as HTMLSelectElement).value).toBe('worker-b');
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes('/cells/focused-cell/review-diff?agent_id=worker-b'))).toBe(true);
    await fireEvent.input(view.getByLabelText('File comment for change.ts'), { target: { value: 'Keep the guard' } });
    await fireEvent.click(view.getByRole('button', { name: 'Send requested changes' }));
    const posts = fetchMock.mock.calls.filter(([, options]) => options?.method === 'POST');
    expect(posts).toHaveLength(1);
    expect(JSON.parse(posts[0][1]!.body as string)).toEqual({
      target_agent_id: 'worker-b', message: 'Requested changes (advisory review):\n\nchange.ts: Keep the guard', submit: true,
    });
    expect(await view.findByRole('region', { name: 'Injection receipt' })).toBeTruthy();
  });

  it('follows a new focus without refetching cells and permits manual picker changes', async () => {
    const view = render(ChangesTab);
    await view.findByText('new');
    ui.setFocusedAgent('worker-a');
    await waitFor(() => expect((view.getByLabelText('Principal') as HTMLSelectElement).value).toBe('worker-a'));
    await fireEvent.change(view.getByLabelText('Principal'), { target: { value: 'worker-c' } });
    await waitFor(() => expect((view.getByLabelText('Principal') as HTMLSelectElement).value).toBe('worker-c'));
    expect(fetchMock.mock.calls.filter(([url]) => String(url).endsWith('/cells'))).toHaveLength(1);
  });

  it('falls back for an unknown focus and makes Files and Artifacts reachable', async () => {
    ui.setFocusedAgent('unknown-agent');
    const view = render(ChangesTab);
    await view.findByText('new');
    expect((view.getByLabelText('Principal') as HTMLSelectElement).value).toBe('worker-a');
    await fireEvent.click(view.getByRole('button', { name: 'Files' }));
    expect(mounts.files).toHaveBeenCalled();
    await fireEvent.click(view.getByRole('button', { name: 'Artifacts' }));
    expect(mounts.artifacts).toHaveBeenCalled();
  });
});
