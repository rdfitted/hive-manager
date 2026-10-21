import { afterEach, describe, expect, it, vi } from 'vitest';
import { createLayoutStore, migrateRightTab, type LayoutState } from './layout';

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
  it('defaults to Overview without saved layout', () => {
    expect(readStore(loadStore(createStorage())).rightTab).toBe('overview');
  });

  it.each([
    ['status', 'overview'],
    ['plan', 'overview'],
    ['graph', 'overview'],
    ['logs', 'activity'],
    ['chat', 'activity'],
    ['timeline', 'activity'],
    ['review', 'changes'],
    ['files', 'changes'],
    ['unknown-tab', 'overview'],
  ])('G-tab-migration: loads saved %s as %s', (rightTab, expected) => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ rightTab, rightCollapsed: true, rightWidth: 400 }),
    });

    expect(readStore(loadStore(storage))).toMatchObject({
      rightTab: expected,
      rightCollapsed: true,
      rightWidth: 400,
    });
  });

  it.each(['overview', 'activity', 'changes'] as const)('preserves saved %s', (rightTab) => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ rightTab }),
    });
    expect(readStore(loadStore(storage)).rightTab).toBe(rightTab);
  });

  it.each([undefined, null, 42, {}, []])('maps malformed tab %j to Overview', (value) => {
    expect(migrateRightTab(value)).toBe('overview');
  });

  it.each([false, true])('setRightTab always opens the active tab when collapsed is %s', (rightCollapsed) => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ rightTab: 'activity', rightCollapsed }),
    });
    const layout = loadStore(storage);
    layout.setRightTab('activity');
    expect(readStore(layout)).toMatchObject({ rightTab: 'activity', rightCollapsed: false });
    expect(readPersistedLayout(storage)).toMatchObject({ rightTab: 'activity', rightCollapsed: false });
  });

  it('collapses the open active right tab without changing the tab', () => {
    const storage = createStorage();
    const layout = loadStore(storage);

    layout.activateRightTab('overview');

    expect(readStore(layout)).toMatchObject({ rightTab: 'overview', rightCollapsed: true });
    expect(readPersistedLayout(storage)).toMatchObject({
      rightTab: 'overview',
      rightCollapsed: true,
    });
  });

  it('expands the active right tab when it is collapsed', () => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ rightTab: 'activity', rightCollapsed: true }),
    });
    const layout = loadStore(storage);

    layout.activateRightTab('activity');

    expect(readStore(layout)).toMatchObject({ rightTab: 'activity', rightCollapsed: false });
  });

  it.each([false, true])(
    'switches to and expands a different right tab when collapsed is %s',
    (rightCollapsed) => {
      const storage = createStorage({
        'hive-manager-layout': JSON.stringify({ rightTab: 'overview', rightCollapsed }),
      });
      const layout = loadStore(storage);

      layout.activateRightTab('activity');

      expect(readStore(layout)).toMatchObject({ rightTab: 'activity', rightCollapsed: false });
      expect(readPersistedLayout(storage)).toMatchObject({
        rightTab: 'activity',
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

describe('session Focus/Grid preference', () => {
  it('defaults to Focus for a new layout', () => {
    expect(readStore(loadStore(createStorage())).sessionViewMode).toBe('focus');
  });

  it('loads an old layout without a view mode as Focus, preserving rail migration', () => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ rightTab: 'chat', leftWidth: 310 }),
    });
    expect(readStore(loadStore(storage))).toMatchObject({
      sessionViewMode: 'focus', rightTab: 'activity', leftWidth: 310,
    });
  });

  it.each(['unknown', '', null, 42, {}, []])('coerces stored mode %j to Focus', (sessionViewMode) => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ sessionViewMode }),
    });
    expect(readStore(loadStore(storage)).sessionViewMode).toBe('focus');
  });

  it.each(['focus', 'grid'] as const)('preserves saved %s mode', (sessionViewMode) => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ sessionViewMode }),
    });
    expect(readStore(loadStore(storage)).sessionViewMode).toBe(sessionViewMode);
  });

  it('persists explicit mode changes across layout store instances', () => {
    const storage = createStorage();
    const layout = loadStore(storage);
    layout.setSessionViewMode('grid');
    expect(readPersistedLayout(storage).sessionViewMode).toBe('grid');
    expect(readStore(createLayoutStore()).sessionViewMode).toBe('grid');
    layout.setSessionViewMode('focus');
    expect(readStore(createLayoutStore()).sessionViewMode).toBe('focus');
  });

  it('toggles and persists the app-wide preference in both directions', () => {
    const storage = createStorage();
    const layout = loadStore(storage);
    layout.toggleSessionViewMode();
    expect(readStore(layout).sessionViewMode).toBe('grid');
    expect(readStore(createLayoutStore()).sessionViewMode).toBe('grid');
    layout.toggleSessionViewMode();
    expect(readStore(layout).sessionViewMode).toBe('focus');
    expect(readPersistedLayout(storage).sessionViewMode).toBe('focus');
  });
});
