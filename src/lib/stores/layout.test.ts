import { afterEach, describe, expect, it, vi } from 'vitest';

function createStorage(initial: Record<string, string> = {}): Storage {
  const values = new Map(Object.entries(initial));

  return {
    get length() {
      return values.size;
    },
    clear() {
      values.clear();
    },
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    key(index: number) {
      return [...values.keys()][index] ?? null;
    },
    removeItem(key: string) {
      values.delete(key);
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };
}

async function loadStore(storage: Storage) {
  vi.stubGlobal('localStorage', storage);
  vi.resetModules();
  return import('./layout');
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('layout terminal maximize state', () => {
  it('resets a persisted maximized terminal when the store loads', async () => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ maximizedTerminalId: 'agent-1' }),
    });
    const { layout } = await loadStore(storage);

    let current: { maximizedTerminalId: string | null } | undefined;
    const unsubscribe = layout.subscribe((state) => {
      current = state;
    });

    expect(current?.maximizedTerminalId).toBeNull();
    unsubscribe();
  });

  it('toggles and persists maximize state during the current app run', async () => {
    const storage = createStorage();
    const { layout } = await loadStore(storage);

    layout.toggleMaximizedTerminal('agent-2');
    expect(JSON.parse(storage.getItem('hive-manager-layout') ?? '{}').maximizedTerminalId).toBe(
      'agent-2',
    );

    layout.toggleMaximizedTerminal('agent-2');
    expect(JSON.parse(storage.getItem('hive-manager-layout') ?? '{}').maximizedTerminalId).toBeNull();
  });
});
