import { afterEach, describe, expect, it, vi } from 'vitest';
import { createLayoutStore, LAYOUT_COMPOSITION, migrateRightTab, type LayoutState } from './layout';

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
    ['plan', 'plan'],
    ['graph', 'graph'],
    ['logs', 'logs'],
    ['chat', 'activity'],
    ['timeline', 'timeline'],
    ['review', 'changes'],
    ['files', 'files'],
    ['artifacts', 'artifacts'],
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
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ rightTab: 'overview', rightCollapsed: false, composition: LAYOUT_COMPOSITION }),
    });
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

  it('round-trips panel state through localStorage', () => {
    const storage = createStorage();
    const layout = loadStore(storage);
    layout.setLeftCollapsed(false);
    layout.toggleRight();

    expect(readStore(createLayoutStore())).toMatchObject({ leftCollapsed: false, rightCollapsed: false });

    layout.setLeftCollapsed(true);
    layout.toggleRight();

    expect(readStore(createLayoutStore())).toMatchObject({ leftCollapsed: true, rightCollapsed: true });
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
  it('opens a new layout on the stage: Grid, the session rail and a closed inspector', () => {
    expect(readStore(loadStore(createStorage()))).toMatchObject({
      sessionViewMode: 'grid', leftCollapsed: true, rightCollapsed: true, composition: LAYOUT_COMPOSITION,
    });
  });

  it('adopts the stage once for a layout saved before it, keeping tab and width migration', () => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({
        rightTab: 'chat', leftWidth: 310, sessionViewMode: 'focus', leftCollapsed: false, rightCollapsed: false,
      }),
    });
    const layout = loadStore(storage);
    expect(readStore(layout)).toMatchObject({
      sessionViewMode: 'grid', leftCollapsed: true, rightCollapsed: true, rightTab: 'activity', leftWidth: 310,
    });

    // Choices made after the adoption persist and are not reset again.
    layout.setSessionViewMode('focus');
    layout.setLeftCollapsed(false);
    expect(readPersistedLayout(storage).composition).toBe(LAYOUT_COMPOSITION);
    expect(readStore(createLayoutStore())).toMatchObject({ sessionViewMode: 'focus', leftCollapsed: false });
  });

  it.each(['unknown', '', null, 42, {}, []])('coerces stored mode %j to Grid', (sessionViewMode) => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ sessionViewMode, composition: LAYOUT_COMPOSITION }),
    });
    expect(readStore(loadStore(storage)).sessionViewMode).toBe('grid');
  });

  it.each(['focus', 'grid'] as const)('preserves saved %s mode', (sessionViewMode) => {
    const storage = createStorage({
      'hive-manager-layout': JSON.stringify({ sessionViewMode, composition: LAYOUT_COMPOSITION }),
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
    expect(readStore(layout).sessionViewMode).toBe('focus');
    expect(readStore(createLayoutStore()).sessionViewMode).toBe('focus');
    layout.toggleSessionViewMode();
    expect(readStore(layout).sessionViewMode).toBe('grid');
    expect(readPersistedLayout(storage).sessionViewMode).toBe('grid');
  });
});
