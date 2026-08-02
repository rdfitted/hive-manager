import { afterEach, describe, expect, it, vi } from 'vitest';
import { createLayoutStore, type LayoutState } from './layout';

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

function loadStore(storage: Storage) {
  vi.stubGlobal('localStorage', storage);
  return createLayoutStore();
}

afterEach(() => {
  vi.unstubAllGlobals();
});

function readPersistedLayout(storage: Storage): Partial<LayoutState> {
  return JSON.parse(storage.getItem('hive-manager-layout') ?? '{}');
}

function readStore(layout: ReturnType<typeof createLayoutStore>): LayoutState {
  let current: LayoutState | undefined;
  const unsubscribe = layout.subscribe((state) => {
    current = state;
  });
  unsubscribe();
  if (!current) throw new Error('Layout store did not emit its current state');
  return current;
}

describe('layout panel state', () => {
  it('collapses the open active right tab without changing the tab', () => {
    const storage = createStorage();
    const layout = loadStore(storage);

    layout.activateRightTab('status');

    expect(readStore(layout)).toMatchObject({ rightTab: 'status', rightCollapsed: true });
    expect(readPersistedLayout(storage)).toMatchObject({
      rightTab: 'status',
      rightCollapsed: true,
    });
  });

  it('expands the active right tab when it is collapsed', () => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ rightTab: 'logs', rightCollapsed: true }),
    });
    const layout = loadStore(storage);

    layout.activateRightTab('logs');

    expect(readStore(layout)).toMatchObject({ rightTab: 'logs', rightCollapsed: false });
  });

  it.each([false, true])(
    'switches to and expands a different right tab when collapsed is %s',
    (rightCollapsed) => {
      const storage = createStorage({
        'hive-manager-layout': JSON.stringify({ rightTab: 'status', rightCollapsed }),
      });
      const layout = loadStore(storage);

      layout.activateRightTab('chat');

      expect(readStore(layout)).toMatchObject({ rightTab: 'chat', rightCollapsed: false });
      expect(readPersistedLayout(storage)).toMatchObject({
        rightTab: 'chat',
        rightCollapsed: false,
      });
    },
  );

  it('persists an absolute left-panel expansion even when already expanded', () => {
    const storage = createStorage();
    const persist = vi.spyOn(storage, 'setItem');
    const layout = loadStore(storage);

    layout.setLeftCollapsed(false);

    expect(persist).toHaveBeenCalledOnce();
    expect(readStore(layout).leftCollapsed).toBe(false);
  });

  it('round-trips collapsed panel state through localStorage', () => {
    const storage = createStorage();
    const layout = loadStore(storage);
    layout.setLeftCollapsed(true);
    layout.toggleRight();

    const restoredLayout = createLayoutStore();

    expect(readStore(restoredLayout)).toMatchObject({
      leftCollapsed: true,
      rightCollapsed: true,
    });
  });
});

describe('layout terminal maximize state', () => {
  it('resets a persisted maximized terminal when the store loads', () => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ maximizedTerminalId: 'agent-1' }),
    });
    const layout = loadStore(storage);

    expect(readStore(layout).maximizedTerminalId).toBeNull();
  });

  it('toggles and persists maximize state during the current app run', () => {
    const storage = createStorage();
    const layout = loadStore(storage);

    layout.toggleMaximizedTerminal('agent-2');
    expect(JSON.parse(storage.getItem('hive-manager-layout') ?? '{}').maximizedTerminalId).toBe(
      'agent-2',
    );

    layout.toggleMaximizedTerminal('agent-2');
    expect(JSON.parse(storage.getItem('hive-manager-layout') ?? '{}').maximizedTerminalId).toBeNull();
  });
});
