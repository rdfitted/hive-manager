import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import type { SessionFileEntry } from './sessionFiles';

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function jsonResponse(payload: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: vi.fn().mockResolvedValue(payload),
  } as unknown as Response;
}

function fileEntry(
  path: string,
  size = 10,
  modified: string | number | null = '2026-07-18T00:00:00Z',
): SessionFileEntry {
  return {
    path,
    name: path.split('/').at(-1) ?? path,
    is_dir: false,
    size,
    modified,
  };
}

describe('sessionFilesStore polling', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.resetModules();
    fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('deduplicates an unsettled poll and permits a later poll after settlement', async () => {
    const { sessionFilesStore } = await import('./sessionFiles');
    const firstResponse = deferred<Response>();
    fetchMock.mockImplementationOnce(() => firstResponse.promise);
    sessionFilesStore.setSessionId('session-a');

    const firstPoll = sessionFilesStore.pollFiles();
    const duplicatePoll = sessionFilesStore.pollFiles();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    await duplicatePoll;

    firstResponse.resolve(jsonResponse({ files: [fileEntry('first.txt')] }));
    await firstPoll;

    fetchMock.mockResolvedValueOnce(jsonResponse({ files: [fileEntry('first.txt')] }));
    await sessionFilesStore.pollFiles();

    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('lets a new session poll while the old poll settles without clearing the new guard', async () => {
    const { sessionFilesStore } = await import('./sessionFiles');
    const sessionAResponse = deferred<Response>();
    const sessionBResponse = deferred<Response>();
    fetchMock
      .mockImplementationOnce(() => sessionAResponse.promise)
      .mockImplementationOnce(() => sessionBResponse.promise);

    sessionFilesStore.setSessionId('session-a');
    const sessionAPoll = sessionFilesStore.pollFiles();

    sessionFilesStore.setSessionId('session-b');
    const sessionBPoll = sessionFilesStore.pollFiles();
    expect(fetchMock).toHaveBeenCalledTimes(2);

    sessionAResponse.resolve(jsonResponse({ files: [fileEntry('stale-a.txt')] }));
    await sessionAPoll;
    await sessionFilesStore.pollFiles();
    expect(fetchMock).toHaveBeenCalledTimes(2);

    sessionBResponse.resolve(jsonResponse({ files: [fileEntry('current-b.txt')] }));
    await sessionBPoll;

    expect(get(sessionFilesStore).sessionId).toBe('session-b');
    expect(get(sessionFilesStore).entries.map((entry) => entry.path)).toEqual(['current-b.txt']);

    fetchMock.mockResolvedValueOnce(
      jsonResponse({ files: [fileEntry('current-b.txt')] }),
    );
    await sessionFilesStore.pollFiles();
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });

  it('re-reads selected content only after metadata changes and guards the content request', async () => {
    const { sessionFilesStore } = await import('./sessionFiles');
    const path = 'notes.txt';
    const initialEntry = fileEntry(path, 10, '2026-07-18T00:00:00Z');
    const changedEntry = fileEntry(path, 20, '2026-07-18T00:01:00Z');
    const refreshedContent = deferred<Response>();

    fetchMock
      .mockResolvedValueOnce(jsonResponse({ files: [initialEntry] }))
      .mockResolvedValueOnce(
        jsonResponse({ path, content: 'initial content', size: initialEntry.size }),
      )
      .mockResolvedValueOnce(jsonResponse({ files: [{ ...initialEntry }] }))
      .mockResolvedValueOnce(jsonResponse({ files: [changedEntry] }))
      .mockImplementationOnce(() => refreshedContent.promise);

    sessionFilesStore.setSessionId('session-a');
    await sessionFilesStore.loadFiles('session-a');
    await sessionFilesStore.selectFile(path);

    await sessionFilesStore.pollFiles();
    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(get(sessionFilesStore).content?.content).toBe('initial content');

    const changedPoll = sessionFilesStore.pollFiles();
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(5));

    await sessionFilesStore.pollFiles();
    expect(fetchMock).toHaveBeenCalledTimes(5);

    const contentUrl = String(fetchMock.mock.calls[4]?.[0]);
    expect(contentUrl).toContain('/files/content?');
    expect(contentUrl).toContain('path=notes.txt');

    refreshedContent.resolve(
      jsonResponse({ path, content: 'refreshed content', size: changedEntry.size }),
    );
    await changedPoll;
    expect(get(sessionFilesStore).content?.content).toBe('refreshed content');

    fetchMock.mockResolvedValueOnce(jsonResponse({ files: [{ ...changedEntry }] }));
    await sessionFilesStore.pollFiles();
    expect(fetchMock).toHaveBeenCalledTimes(6);
  });

  it('does not replace a user content request when selection changes during a poll', async () => {
    const { sessionFilesStore } = await import('./sessionFiles');
    const firstPath = 'first.txt';
    const secondPath = 'second.txt';
    const entries = [
      fileEntry(firstPath, 10, '2026-07-18T00:00:00Z'),
      fileEntry(secondPath, 20, '2026-07-18T00:01:00Z'),
    ];
    const pollResponse = deferred<Response>();

    fetchMock
      .mockResolvedValueOnce(jsonResponse({ files: entries }))
      .mockResolvedValueOnce(
        jsonResponse({ path: firstPath, content: 'first content', size: 10 }),
      )
      .mockImplementationOnce(() => pollResponse.promise)
      .mockResolvedValueOnce(
        jsonResponse({ path: secondPath, content: 'second content', size: 20 }),
      );

    sessionFilesStore.setSessionId('session-a');
    await sessionFilesStore.loadFiles('session-a');
    await sessionFilesStore.selectFile(firstPath);

    const poll = sessionFilesStore.pollFiles();
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(3));

    await sessionFilesStore.selectFile(secondPath);
    expect(fetchMock).toHaveBeenCalledTimes(4);

    pollResponse.resolve(jsonResponse({ files: entries.map((entry) => ({ ...entry })) }));
    await poll;

    expect(fetchMock).toHaveBeenCalledTimes(4);
    expect(get(sessionFilesStore).selectedPath).toBe(secondPath);
    expect(get(sessionFilesStore).content?.content).toBe('second content');
  });
});
