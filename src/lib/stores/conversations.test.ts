import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

import { conversationStore, type ConversationMessage } from './conversations';

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
