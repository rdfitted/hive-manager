import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { fetchCliHealth } from './AgentConfigEditor.svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  isTauri: vi.fn(),
}));

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;
const isTauriMock = isTauri as unknown as ReturnType<typeof vi.fn>;

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function jsonResponse(payload: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: vi.fn().mockResolvedValue(payload),
  } as unknown as Response;
}

describe('fetchCliHealth', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    invokeMock.mockReset();
    isTauriMock.mockReset();
    fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('uses Tauri, normalizes, deduplicates, caches, and refreshes on force', async () => {
    isTauriMock.mockReturnValue(true);
    const firstRequest = deferred<unknown>();
    invokeMock
      .mockReturnValueOnce(firstRequest.promise)
      .mockResolvedValueOnce({
        clis: [{
          cli: 'codex',
          resolved: true,
          binPath: 'C:\\Tools\\codex.exe',
          loggedIn: 'yes',
          detail: 'Refreshed',
          staleHint: false,
        }],
      });

    const first = fetchCliHealth(true);
    const concurrent = fetchCliHealth(true);

    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith('get_cli_health');
    expect(fetchMock).not.toHaveBeenCalled();

    firstRequest.resolve({
      clis: [{
        cli: 'codex',
        resolved: true,
        binPath: 'C:\\Tools\\codex.exe',
        loggedIn: 'yes',
        detail: 'Ready',
        staleHint: false,
      }],
    });

    const [firstHealth, concurrentHealth] = await Promise.all([first, concurrent]);
    expect(firstHealth).toEqual(concurrentHealth);
    expect(firstHealth.codex).toEqual({
      cli: 'codex',
      resolved: true,
      binPath: 'C:\\Tools\\codex.exe',
      loggedIn: 'yes',
      detail: 'Ready',
      staleHint: false,
    });

    expect(await fetchCliHealth()).toBe(firstHealth);
    expect(invokeMock).toHaveBeenCalledTimes(1);

    const refreshed = await fetchCliHealth(true);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(refreshed.codex.detail).toBe('Refreshed');
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('uses HTTP, normalizes, deduplicates, caches, and refreshes on force', async () => {
    isTauriMock.mockReturnValue(false);
    const firstRequest = deferred<Response>();
    fetchMock
      .mockReturnValueOnce(firstRequest.promise)
      .mockResolvedValueOnce(jsonResponse({
        clis: {
          gemini: {
            name: 'gemini',
            resolved: true,
            binPath: '/usr/bin/gemini',
            loggedIn: 'yes',
            detail: 'Refreshed',
            staleHint: false,
          },
        },
      }));

    const first = fetchCliHealth(true);
    const concurrent = fetchCliHealth(true);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledWith(expect.stringMatching(/\/api\/cli-health$/));
    expect(invokeMock).not.toHaveBeenCalled();

    firstRequest.resolve(jsonResponse({
      clis: {
        gemini: {
          name: 'gemini',
          resolved: true,
          binPath: '/usr/bin/gemini',
          loggedIn: 'yes',
          detail: 'Ready',
          staleHint: false,
        },
      },
    }));

    const [firstHealth, concurrentHealth] = await Promise.all([first, concurrent]);
    expect(firstHealth).toEqual(concurrentHealth);
    expect(firstHealth.gemini).toEqual({
      cli: 'gemini',
      resolved: true,
      binPath: '/usr/bin/gemini',
      loggedIn: 'yes',
      detail: 'Ready',
      staleHint: false,
    });

    expect(await fetchCliHealth()).toBe(firstHealth);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    const refreshed = await fetchCliHealth(true);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(refreshed.gemini.detail).toBe('Refreshed');
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('surfaces transport and normalized-response errors on both paths', async () => {
    isTauriMock.mockReturnValue(true);
    invokeMock.mockRejectedValueOnce(new Error('desktop health unavailable'));
    await expect(fetchCliHealth(true)).rejects.toThrow('desktop health unavailable');

    invokeMock.mockResolvedValueOnce({ clis: [] });
    await expect(fetchCliHealth(true)).rejects.toThrow('CLI health response was empty');

    isTauriMock.mockReturnValue(false);
    fetchMock.mockResolvedValueOnce(jsonResponse({ message: 'unavailable' }, 503));
    await expect(fetchCliHealth(true)).rejects.toThrow('CLI health request failed (503)');

    fetchMock.mockResolvedValueOnce(jsonResponse({ clis: [] }));
    await expect(fetchCliHealth(true)).rejects.toThrow('CLI health response was empty');
  });
});
