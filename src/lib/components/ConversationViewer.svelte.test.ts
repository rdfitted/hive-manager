import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { writable } from 'svelte/store';
import { tick } from 'svelte';
import type { Session } from '$lib/stores/sessions';
import type { ConversationMessage } from '$lib/stores/conversations';

const { handlers, invokeMock } = vi.hoisted(() => ({
  handlers: new Map<string, (event: { payload: unknown }) => void>(),
  invokeMock: vi.fn(),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn((name, handler) => {
  handlers.set(name, handler);
  return Promise.resolve(() => {});
}) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('$lib/stores/sessions', async (original) => ({
  ...await original<typeof import('$lib/stores/sessions')>(),
  activeSession: (await import('svelte/store')).writable(null),
}));
import { activeSession } from '$lib/stores/sessions';
import { conversationStore } from '$lib/stores/conversations';
import ConversationViewer from './ConversationViewer.svelte';

const fetchMock = vi.fn();
let saved: ConversationMessage[] = [];
const diff: ConversationMessage = {
  timestamp: '2026-01-02T03:04:05Z', from: 'worker-1', content: 'diff --git a/demo b/demo\n@@ -1 +1 @@\n-old\n+new',
  renderer: 'diff', data: { unified: 'diff --git a/demo b/demo\n@@ -1 +1 @@\n-old\n+new' },
  session_id: 'render-session', agent_id: 'shared',
};

beforeEach(() => {
  saved = [];
  invokeMock.mockReset().mockResolvedValue({ delivered: true });
  fetchMock.mockReset().mockImplementation(async (input) => ({ ok: true, json: async () =>
    String(input).includes('/conversations/') ? { messages: saved } : { sessions: [] },
  }));
  vi.stubGlobal('fetch', fetchMock);
  conversationStore.setSessionId(null);
  (activeSession as ReturnType<typeof writable<Session | null>>).set({ id: 'render-session', agents: [] } as unknown as Session);
});
afterEach(() => {
  cleanup();
  conversationStore.setSessionId(null);
  (activeSession as ReturnType<typeof writable<Session | null>>).set(null);
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.clearAllTimers();
  vi.useRealTimers();
});

// Preserve the existing tab-selection and polling guards, now against the real store.
describe('ConversationViewer tab selection', () => {
  function workerSession() {
    (activeSession as ReturnType<typeof writable<Session | null>>).set({
      id: 'render-session', agents: [{ id: 'worker-1', role: { Worker: { index: 1 } }, status: 'Running', config: { label: 'Worker One' } }],
    } as unknown as Session);
    saved = [{ timestamp: diff.timestamp, from: 'worker-1', content: 'Loaded conversation' }];
  }

  it('selects the clicked agent in the conversation store', async () => {
    workerSession();
    const select = vi.spyOn(conversationStore, 'selectAgent');
    const view = render(ConversationViewer);
    await fireEvent.click(view.getByRole('tab', { name: 'Worker One' }));
    expect(select).toHaveBeenCalledWith('worker-1');
    expect(view.getByRole('textbox', { name: /Send message as operator/ })).toBeTruthy();
    expect(view.getByRole('tab', { name: 'Worker One' }).getAttribute('aria-selected')).toBe('true');
  });

  it('selects the agent strictly before loading its conversation', async () => {
    workerSession();
    const select = vi.spyOn(conversationStore, 'selectAgent');
    const load = vi.spyOn(conversationStore, 'loadConversation');
    const view = render(ConversationViewer);
    await fireEvent.click(view.getByRole('tab', { name: 'Worker One' }));
    expect(load).toHaveBeenCalledWith('render-session', 'worker-1');
    expect(select.mock.invocationCallOrder[0]).toBeLessThan(load.mock.invocationCallOrder[0]);
  });

  it('renders the selected conversation only after the store selection gate is established', async () => {
    workerSession();
    const view = render(ConversationViewer);
    await fireEvent.click(view.getByRole('tab', { name: 'Worker One' }));
    await waitFor(() => expect(view.getByText('Loaded conversation')).toBeTruthy());
    expect(view.queryByText('Select an agent tab to view conversation.')).toBeNull();
  });

  it('polls an already-selected conversation without raising the loading skeleton', async () => {
    vi.useFakeTimers();
    conversationStore.setSessionId('render-session');
    conversationStore.selectAgent('shared');
    const load = vi.spyOn(conversationStore, 'loadConversation');
    render(ConversationViewer);
    await tick();
    load.mockClear();
    vi.advanceTimersByTime(5000);
    await tick();
    expect(load).toHaveBeenCalledWith('render-session', 'shared', undefined, { silent: true });
  });
});

describe('G57 conversation widgets', () => {
  it('renders a real diff envelope live and after GET reload', async () => {
    const view = render(ConversationViewer);
    await fireEvent.click(view.getByRole('tab', { name: 'Shared' }));
    await waitFor(() => expect(view.getByText('No messages yet.')).toBeTruthy());
    handlers.get('conversation-message')?.({ payload: diff });
    await waitFor(() => expect(view.container.querySelector('.diff-line.add')?.textContent).toContain('new'));
    saved = [diff];
    conversationStore.selectAgent('shared');
    await conversationStore.loadConversation('render-session', 'shared');
    await waitFor(() => expect(view.container.querySelector('.diff-line.del')?.textContent).toContain('old'));
    expect(view.container.querySelectorAll('.diff-widget')).toHaveLength(1);
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes('/conversations/shared'))).toBe(true);
  });

  it.each(['approved', 'rejected'] as const)('sends a structured %s decision bound only to the server ID', async (decision) => {
    saved = [{ ...diff, renderer: 'approval', content: 'Review patch', data: { title: 'Review patch', approvalId: 'pending-demo' } }];
    const view = render(ConversationViewer);
    await fireEvent.click(view.getByRole('tab', { name: 'Shared' }));
    await waitFor(() => expect(view.getByTestId('approve')).toBeTruthy());
    await fireEvent.click(view.getByTestId(decision === 'approved' ? 'approve' : 'reject'));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('resolve_approval', { request: {
      session_id: 'render-session', approval_id: 'pending-demo', decision,
    } }));
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect((view.getByTestId('approve') as HTMLButtonElement).disabled).toBe(true);
    expect((view.getByTestId('reject') as HTMLButtonElement).disabled).toBe(true);
  });

  it('shows a stale decision error returned by the backend', async () => {
    invokeMock.mockRejectedValue('Approval is stale or already resolved');
    saved = [{ ...diff, renderer: 'approval', data: { title: 'Review patch', approvalId: 'expired-demo' } }];
    const view = render(ConversationViewer);
    await fireEvent.click(view.getByRole('tab', { name: 'Shared' }));
    await waitFor(() => expect(view.getByTestId('approve')).toBeTruthy());
    await fireEvent.click(view.getByTestId('approve'));
    await waitFor(() => expect(view.getByText('Approval is stale or already resolved')).toBeTruthy());
    expect((view.getByTestId('approve') as HTMLButtonElement).disabled).toBe(true);
    expect((view.getByTestId('reject') as HTMLButtonElement).disabled).toBe(true);
  });

  it('shows the authenticated requester on a reloaded approval card', async () => {
    saved = [{ ...diff, renderer: 'approval', data: { title: 'Review patch', approvalId: 'pending-demo', requesterAgentId: 'render-session-worker-1' } }];
    const view = render(ConversationViewer);
    await fireEvent.click(view.getByRole('tab', { name: 'Shared' }));
    await waitFor(() => expect(view.getByText('Requested by render-session-worker-1')).toBeTruthy());
  });

  it('keeps both buttons disabled until the decision invoke settles', async () => {
    let finish!: () => void;
    invokeMock.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    saved = [{ ...diff, renderer: 'approval', data: { approvalId: 'pending-demo' } }];
    const view = render(ConversationViewer);
    await fireEvent.click(view.getByRole('tab', { name: 'Shared' }));
    await waitFor(() => expect(view.getByTestId('approve')).toBeTruthy());
    await fireEvent.click(view.getByTestId('approve'));
    expect((view.getByTestId('approve') as HTMLButtonElement).disabled).toBe(true);
    expect((view.getByTestId('reject') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(view.getByTestId('reject'));
    expect(invokeMock).toHaveBeenCalledTimes(1);
    finish();
    await tick();
    expect((view.getByTestId('approve') as HTMLButtonElement).disabled).toBe(true);
  });

  it('re-enables both buttons after a retryable invoke failure and permits a retry', async () => {
    invokeMock.mockRejectedValueOnce('Temporary invoke failure').mockResolvedValueOnce({ delivered: true });
    saved = [{ ...diff, renderer: 'approval', data: { approvalId: 'pending-demo' } }];
    const view = render(ConversationViewer);
    await fireEvent.click(view.getByRole('tab', { name: 'Shared' }));
    await waitFor(() => expect(view.getByTestId('approve')).toBeTruthy());
    await fireEvent.click(view.getByTestId('approve'));
    await waitFor(() => expect(view.getByText('Temporary invoke failure')).toBeTruthy());
    expect((view.getByTestId('approve') as HTMLButtonElement).disabled).toBe(false);
    expect((view.getByTestId('reject') as HTMLButtonElement).disabled).toBe(false);
    await fireEvent.click(view.getByTestId('reject'));
    await waitFor(() => expect(invokeMock).toHaveBeenLastCalledWith('resolve_approval', { request: {
      session_id: 'render-session', approval_id: 'pending-demo', decision: 'rejected',
    } }));
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect((view.getByTestId('reject') as HTMLButtonElement).disabled).toBe(true);
    expect(view.queryByText('Temporary invoke failure')).toBeNull();
  });

  it('keeps a consumed approval disabled after an ambiguous delivery failure', async () => {
    invokeMock.mockRejectedValue('[approval_consumed] Delivery acknowledgement failed');
    saved = [{ ...diff, renderer: 'approval', data: { approvalId: 'pending-demo' } }];
    const view = render(ConversationViewer);
    await fireEvent.click(view.getByRole('tab', { name: 'Shared' }));
    await waitFor(() => expect(view.getByTestId('approve')).toBeTruthy());
    await fireEvent.click(view.getByTestId('approve'));
    await waitFor(() => expect(view.getByText('Delivery acknowledgement failed')).toBeTruthy());
    expect((view.getByTestId('approve') as HTMLButtonElement).disabled).toBe(true);
    expect((view.getByTestId('reject') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(view.getByTestId('reject'));
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});
