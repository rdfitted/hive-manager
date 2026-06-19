import { beforeEach, afterEach, describe, it, expect, vi } from 'vitest';

/**
 * composerDraft tests run in jsdom (filename includes `.svelte.test`? no — these are pure
 * store tests that only need a localStorage stub). We provide a Map-backed localStorage so
 * the node environment has the API the store guards on.
 */

function installLocalStorage(impl?: Partial<Storage>) {
  const map = new Map<string, string>();
  const store: Storage = {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (k: string) => (map.has(k) ? map.get(k)! : null),
    key: (i: number) => Array.from(map.keys())[i] ?? null,
    removeItem: (k: string) => map.delete(k),
    setItem: (k: string, v: string) => {
      map.set(k, v);
    },
    ...impl,
  };
  (globalThis as unknown as { localStorage: Storage }).localStorage = store;
  return { map, store };
}

describe('composerDraft store', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
    installLocalStorage();
  });

  afterEach(() => {
    vi.useRealTimers();
    delete (globalThis as unknown as { localStorage?: Storage }).localStorage;
  });

  it('writes a draft and reloads it from localStorage (roundtrip)', async () => {
    const { composerDraft, draftKey } = await import('./composerDraft');

    composerDraft.load('s1');
    composerDraft.update('hello world');
    // Debounced write — advance past the debounce window.
    vi.advanceTimersByTime(500);

    expect(localStorage.getItem(draftKey('s1'))).toBe('hello world');

    // Re-binding the session hydrates the persisted text.
    composerDraft.load('s2'); // bind elsewhere first
    const restored = composerDraft.load('s1');
    expect(restored).toBe('hello world');
  });

  it('falls back to in-memory when localStorage.setItem throws (quota) without throwing', async () => {
    installLocalStorage({
      setItem: () => {
        throw new DOMException('QuotaExceededError');
      },
    });
    const { composerDraft } = await import('./composerDraft');

    composerDraft.load('quota');
    expect(() => {
      composerDraft.update('big draft');
      vi.advanceTimersByTime(500);
    }).not.toThrow();

    // In-memory fallback still returns the value via read().
    expect(composerDraft.read('quota')).toBe('big draft');
  });

  it('keeps per-session keys isolated', async () => {
    const { composerDraft, draftKey } = await import('./composerDraft');

    composerDraft.load('alpha');
    composerDraft.update('alpha draft');
    vi.advanceTimersByTime(500);

    composerDraft.load('beta');
    composerDraft.update('beta draft');
    vi.advanceTimersByTime(500);

    expect(localStorage.getItem(draftKey('alpha'))).toBe('alpha draft');
    expect(localStorage.getItem(draftKey('beta'))).toBe('beta draft');
    expect(draftKey('alpha')).not.toBe(draftKey('beta'));

    // Loading alpha does not see beta's text.
    expect(composerDraft.load('alpha')).toBe('alpha draft');
  });

  it('clear() removes the persisted draft for the bound session', async () => {
    const { composerDraft, draftKey } = await import('./composerDraft');
    composerDraft.load('c1');
    composerDraft.update('temp');
    vi.advanceTimersByTime(500);
    expect(localStorage.getItem(draftKey('c1'))).toBe('temp');

    composerDraft.clear();
    expect(localStorage.getItem(draftKey('c1'))).toBeNull();
  });
});
