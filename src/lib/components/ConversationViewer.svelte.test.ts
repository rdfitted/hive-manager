import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import { tick } from 'svelte';

interface TestConversationState {
  messages: Array<{ timestamp: string; from: string; content: string }>;
  loading: boolean;
  error: string | null;
  selectedAgent: string | null;
  sessionId: string | null;
}

const storeMocks = vi.hoisted(() => ({
  setConversationState: undefined as
    | ((state: TestConversationState) => void)
    | undefined,
  selectAgent: vi.fn(),
  setSessionId: vi.fn(),
  loadConversation: vi.fn().mockResolvedValue(undefined),
  sendMessage: vi.fn().mockResolvedValue(undefined),
  clearError: vi.fn(),
  loadHeartbeats: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

vi.mock('$lib/stores/sessions', async () => {
  const { derived, writable } = await import('svelte/store');
  const activeSession = writable({
    id: 'session-a',
    agents: [
      {
        id: 'worker-1',
        role: { Worker: { index: 1 } },
        status: 'Running',
        config: { label: 'Worker One' },
      },
    ],
  });
  return {
    activeSession,
    activeAgents: derived(activeSession, (session) => session?.agents ?? []),
    sessions: { subscribe: writable([]).subscribe },
  };
});

vi.mock('$lib/stores/conversations', async () => {
  const { writable } = await import('svelte/store');
  const initialState: TestConversationState = {
    messages: [],
    loading: false,
    error: null,
    selectedAgent: null,
    sessionId: 'session-a',
  };
  const conversationState = writable(initialState);
  let currentState = initialState;
  conversationState.subscribe((state) => {
    currentState = state;
  });
  storeMocks.setConversationState = conversationState.set;
  storeMocks.setSessionId.mockImplementation((sessionId: string | null) => {
    if (currentState.sessionId === sessionId) return;
    conversationState.set({ ...initialState, sessionId });
  });
  storeMocks.selectAgent.mockImplementation((agentId: string | null) => {
    conversationState.update((state) => ({
      ...state,
      selectedAgent: agentId,
      messages: [],
      loading: false,
      error: null,
    }));
  });
  storeMocks.loadConversation.mockImplementation(async (sessionId: string, agentId: string) => {
    if (currentState.sessionId !== sessionId || currentState.selectedAgent !== agentId) return;
    conversationState.update((state) => ({
      ...state,
      messages: [{
        timestamp: '2026-08-01T20:00:00Z',
        from: 'worker-1',
        content: 'Loaded conversation',
      }],
      loading: false,
    }));
  });
  const heartbeatState = writable({
    agents: {},
    stalledAgents: new Set<string>(),
    staleAgents: new Set<string>(),
  });

  return {
    conversationStore: {
      subscribe: conversationState.subscribe,
      selectAgent: storeMocks.selectAgent,
      setSessionId: storeMocks.setSessionId,
      loadConversation: storeMocks.loadConversation,
      sendMessage: storeMocks.sendMessage,
      clearError: storeMocks.clearError,
    },
    heartbeatStore: {
      subscribe: heartbeatState.subscribe,
      loadHeartbeats: storeMocks.loadHeartbeats,
    },
  };
});

import ConversationViewer from './ConversationViewer.svelte';

beforeEach(() => {
  vi.clearAllMocks();
  storeMocks.setConversationState?.({
    messages: [],
    loading: false,
    error: null,
    selectedAgent: null,
    sessionId: 'session-a',
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe('ConversationViewer tab selection', () => {
  it('selects the clicked agent in the conversation store', async () => {
    const { getByRole } = render(ConversationViewer);
    await tick();

    await fireEvent.click(getByRole('tab', { name: 'Worker One' }));

    expect(storeMocks.selectAgent).toHaveBeenCalledWith('worker-1');
    expect(getByRole('textbox', { name: /Send message as operator/ })).toBeTruthy();
    expect(getByRole('tab', { name: 'Worker One' }).getAttribute('aria-selected')).toBe('true');
  });

  it('selects the agent strictly before loading its conversation', async () => {
    const { getByRole } = render(ConversationViewer);
    await tick();

    await fireEvent.click(getByRole('tab', { name: 'Worker One' }));

    expect(storeMocks.loadConversation).toHaveBeenCalledWith('session-a', 'worker-1');
    expect(storeMocks.selectAgent.mock.invocationCallOrder[0]).toBeLessThan(
      storeMocks.loadConversation.mock.invocationCallOrder[0],
    );
  });

  it('renders the selected conversation only after the store selection gate is established', async () => {
    const { getByRole, getByText, queryByText } = render(ConversationViewer);
    await tick();

    await fireEvent.click(getByRole('tab', { name: 'Worker One' }));
    await tick();

    expect(getByText('Loaded conversation')).toBeTruthy();
    expect(queryByText('Select an agent tab to view conversation.')).toBeNull();
  });

  it('polls an already-selected conversation without raising the loading skeleton', async () => {
    vi.useFakeTimers();
    storeMocks.setConversationState?.({
      messages: [],
      loading: false,
      error: null,
      selectedAgent: 'worker-1',
      sessionId: 'session-a',
    });
    render(ConversationViewer);
    await tick();
    storeMocks.loadConversation.mockClear();

    vi.advanceTimersByTime(5000);
    await tick();

    expect(storeMocks.loadConversation).toHaveBeenCalledWith(
      'session-a',
      'worker-1',
      undefined,
      { silent: true },
    );
  });
});
