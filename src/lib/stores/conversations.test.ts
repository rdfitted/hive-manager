import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

import { conversationStore, heartbeatStore, type ConversationMessage } from './conversations';

const fetchMock = vi.fn();

function responseWith(messages: ConversationMessage[]): Response {
  return {
    ok: true,
    status: 200,
    json: vi.fn().mockResolvedValue({ messages }),
  } as unknown as Response;
}

function deferredResponse(): {
  promise: Promise<Response>;
  resolve: (response: Response) => void;
} {
  let resolve!: (response: Response) => void;
  const promise = new Promise<Response>((fulfill) => {
    resolve = fulfill;
  });
  return { promise, resolve };
}

function message(content = 'hello'): ConversationMessage {
  return {
    id: `message-${content}`,
    timestamp: '2026-08-01T20:00:00Z',
    from: 'worker-1',
    content,
  };
}

beforeEach(() => {
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
  conversationStore.setSessionId(`reset-${Math.random()}`);
});

describe('G31 heartbeat work state', () => {
  it('G31 confirmed follow-up gets three minutes after input even with an old completed heartbeat', async () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date('2026-11-01T12:00:00Z'));
      const latest_input_at = new Date().toISOString();
      fetchMock.mockResolvedValue({ ok: true, json: async () => ({ sessions: [{ id: 's', agents: [
        { id: 'confirmed', last_activity: '2026-11-01T11:50:00Z', status: 'completed',
          work_state: 'working', latest_input_at },
        { id: 'legacy', last_activity: '2026-11-01T11:50:00Z', work_state: 'working' },
      ] }] }) });
      await heartbeatStore.loadHeartbeats('s');
      expect([...get(heartbeatStore).staleAgents]).toEqual(['legacy']);
      expect(get(heartbeatStore).agents.confirmed).toHaveProperty('latest_input_at', latest_input_at);
      vi.advanceTimersByTime(180_000);
      await heartbeatStore.loadHeartbeats('s');
      expect(get(heartbeatStore).staleAgents.has('confirmed')).toBe(false);
      vi.advanceTimersByTime(1);
      await heartbeatStore.loadHeartbeats('s');
      expect(get(heartbeatStore).staleAgents.has('confirmed')).toBe(true);
    } finally { vi.useRealTimers(); }
  });
  it('silences completed work, preserves unknown/missing stale work, and re-arms resumed work', async () => {
    const old = new Date(Date.now() - 4 * 60_000).toISOString();
    let seats = [
      { id: 'finished', work_state: 'finished', status: 'completed', last_activity: old },
      { id: 'retired', work_state: 'retired', status: 'completed', last_activity: old },
      { id: 'working', work_state: 'working', status: 'working', last_activity: old },
      { id: 'unknown', work_state: 'future-state', status: 'completed', last_activity: old },
      { id: 'missing', status: 'completed', last_activity: old },
    ];
    fetchMock.mockImplementation(async () => ({ ok: true, json: async () => ({ sessions: [{ id: 's', agents: seats }] }) }));
    await heartbeatStore.loadHeartbeats('s');
    expect([...get(heartbeatStore).staleAgents].sort()).toEqual(['missing', 'unknown', 'working']);
    expect(get(heartbeatStore).agents.finished.work_state).toBe('finished');
    seats = [{ id: 'finished', work_state: 'working', status: 'working', last_activity: old }];
    await heartbeatStore.loadHeartbeats('s');
    expect(get(heartbeatStore).staleAgents.has('finished')).toBe(true);
  });
  it('counts staleness from the later of heartbeat and pending injection', async () => {
    const old = new Date(Date.now() - 4 * 60_000).toISOString();
    const recent = new Date(Date.now() - 30_000).toISOString();
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ sessions: [{ id: 's', agents: [
      { id: 'injected', last_activity: old, awaiting_injected_turn_at: recent, work_state: 'working' },
      { id: 'heartbeat', last_activity: recent, awaiting_injected_turn_at: old, work_state: 'working' },
      { id: 'stalled', last_activity: old, awaiting_injected_turn_at: old, work_state: 'working' },
      { id: 'no-heartbeat', awaiting_injected_turn_at: old, work_state: 'working' },
    ] }] }) });
    await heartbeatStore.loadHeartbeats('s');
    expect([...get(heartbeatStore).staleAgents].sort()).toEqual(['no-heartbeat', 'stalled']);
  });
});

describe('conversationStore conversation selection', () => {
  it('stores messages after selecting the matching agent before loading', async () => {
    fetchMock.mockResolvedValue(responseWith([message()]));
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');

    await conversationStore.loadConversation('session-a', 'worker-1');

    expect(get(conversationStore).messages).toEqual([message()]);
  });

  it('discards a response for an agent that is not selected', async () => {
    fetchMock.mockResolvedValue(responseWith([message('wrong agent')]));
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');

    await conversationStore.loadConversation('session-a', 'worker-2');

    expect(get(conversationStore)).toMatchObject({
      selectedAgent: 'worker-1',
      messages: [],
      loading: false,
    });
  });

  it('preserves the selected agent and messages for an unchanged session id', async () => {
    fetchMock.mockResolvedValueOnce(responseWith([message()]));
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');
    await conversationStore.loadConversation('session-a', 'worker-1');

    const pendingResponse = deferredResponse();
    fetchMock.mockReturnValueOnce(pendingResponse.promise);
    const load = conversationStore.loadConversation('session-a', 'worker-1');
    conversationStore.setSessionId('session-a');

    expect(get(conversationStore)).toMatchObject({
      sessionId: 'session-a',
      selectedAgent: 'worker-1',
      messages: [message()],
    });

    pendingResponse.resolve(responseWith([message('updated')]));
    await load;
    expect(get(conversationStore).messages).toEqual([message('updated')]);
  });

  it('resets the selected agent and messages when the session id changes', async () => {
    fetchMock.mockResolvedValue(responseWith([message()]));
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');
    await conversationStore.loadConversation('session-a', 'worker-1');

    conversationStore.setSessionId('session-b');

    expect(get(conversationStore)).toMatchObject({
      sessionId: 'session-b',
      selectedAgent: null,
      messages: [],
      loading: false,
    });
  });
});

describe('conversationStore loading intent', () => {
  it('does not raise loading during a silent refresh with existing messages', async () => {
    fetchMock.mockResolvedValueOnce(responseWith([message()]));
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');
    await conversationStore.loadConversation('session-a', 'worker-1');

    const pendingResponse = deferredResponse();
    fetchMock.mockReturnValueOnce(pendingResponse.promise);
    const load = conversationStore.loadConversation(
      'session-a',
      'worker-1',
      undefined,
      { silent: true },
    );

    expect(get(conversationStore).loading).toBe(false);
    pendingResponse.resolve(responseWith([message()]));
    await load;
  });

  it('does not raise loading during a silent refresh of an empty conversation', async () => {
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');
    const pendingResponse = deferredResponse();
    fetchMock.mockReturnValueOnce(pendingResponse.promise);

    const load = conversationStore.loadConversation(
      'session-a',
      'worker-1',
      undefined,
      { silent: true },
    );

    expect(get(conversationStore)).toMatchObject({ messages: [], loading: false });
    pendingResponse.resolve(responseWith([]));
    await load;
  });

  it('raises loading for an operator-initiated, non-silent load', async () => {
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');
    const pendingResponse = deferredResponse();
    fetchMock.mockReturnValueOnce(pendingResponse.promise);

    const load = conversationStore.loadConversation('session-a', 'worker-1');

    expect(get(conversationStore).loading).toBe(true);
    pendingResponse.resolve(responseWith([]));
    await load;
    expect(get(conversationStore).loading).toBe(false);
  });

  it('preserves an existing error while a silent refresh is pending and clears it on success', async () => {
    fetchMock.mockRejectedValueOnce(new Error('existing failure'));
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');
    await conversationStore.loadConversation('session-a', 'worker-1');

    const pendingResponse = deferredResponse();
    fetchMock.mockReturnValueOnce(pendingResponse.promise);
    const load = conversationStore.loadConversation(
      'session-a',
      'worker-1',
      undefined,
      { silent: true },
    );

    expect(get(conversationStore).error).toBe('Error: existing failure');
    pendingResponse.resolve(responseWith([]));
    await load;
    expect(get(conversationStore).error).toBeNull();
  });

  it('clears an existing error after a successful silent refresh', async () => {
    fetchMock.mockRejectedValueOnce(new Error('transient failure'));
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');
    await conversationStore.loadConversation('session-a', 'worker-1');
    fetchMock.mockResolvedValueOnce(responseWith([]));

    await conversationStore.loadConversation(
      'session-a',
      'worker-1',
      undefined,
      { silent: true },
    );

    expect(get(conversationStore).error).toBeNull();
  });

  it('surfaces a silent refresh failure and clears it after recovery', async () => {
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');
    fetchMock.mockRejectedValueOnce(new Error('silent failure'));

    await conversationStore.loadConversation(
      'session-a',
      'worker-1',
      undefined,
      { silent: true },
    );

    expect(get(conversationStore).error).toBe('Error: silent failure');

    fetchMock.mockResolvedValueOnce(responseWith([]));
    await conversationStore.loadConversation(
      'session-a',
      'worker-1',
      undefined,
      { silent: true },
    );
    expect(get(conversationStore).error).toBeNull();
  });

  it('polls the selected conversation silently from its last timestamp', async () => {
    fetchMock.mockResolvedValueOnce(responseWith([message()]));
    conversationStore.setSessionId('session-a');
    conversationStore.selectAgent('worker-1');
    await conversationStore.loadConversation('session-a', 'worker-1');
    const loadSpy = vi.spyOn(conversationStore, 'loadConversation').mockResolvedValue(undefined);

    await conversationStore.pollMessages();

    expect(loadSpy).toHaveBeenCalledWith(
      'session-a',
      'worker-1',
      message().timestamp,
      { silent: true },
    );
    loadSpy.mockRestore();
  });
});
