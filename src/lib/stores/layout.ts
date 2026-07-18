import { writable } from 'svelte/store';

export type RightPanelTab = 'status' | 'plan' | 'logs' | 'chat' | 'timeline' | 'files';

export interface LayoutState {
  leftCollapsed: boolean;
  leftWidth: number;
  rightCollapsed: boolean;
  rightWidth: number;
  rightTab: RightPanelTab;
  sessionsCollapsed: boolean;
  recentCollapsed: boolean;
  agentsCollapsed: boolean;
  maximizedTerminalId: string | null;
}

const STORAGE_KEY = 'hive-manager-layout';

export const LEFT_WIDTH_MIN = 200;
export const LEFT_WIDTH_MAX = 420;
export const RIGHT_WIDTH_MIN = 260;
export const RIGHT_WIDTH_MAX = 560;
export const RAIL_WIDTH = 52;

const defaultLayout: LayoutState = {
  leftCollapsed: false,
  leftWidth: 250,
  rightCollapsed: false,
  rightWidth: 320,
  rightTab: 'status',
  sessionsCollapsed: false,
  recentCollapsed: true,
  agentsCollapsed: false,
  maximizedTerminalId: null,
};

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function loadInitial(): LayoutState {
  if (typeof localStorage === 'undefined') return defaultLayout;
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (!stored) return defaultLayout;
    const parsed = JSON.parse(stored) as Partial<LayoutState>;
    return {
      ...defaultLayout,
      ...parsed,
      leftWidth: clamp(parsed.leftWidth ?? defaultLayout.leftWidth, LEFT_WIDTH_MIN, LEFT_WIDTH_MAX),
      rightWidth: clamp(parsed.rightWidth ?? defaultLayout.rightWidth, RIGHT_WIDTH_MIN, RIGHT_WIDTH_MAX),
      // Maximizing is intentionally transient: reopening the app always restores the grid.
      maximizedTerminalId: null,
    };
  } catch {
    return defaultLayout;
  }
}

function createLayoutStore() {
  const { subscribe, update } = writable<LayoutState>(loadInitial());

  function updateAndPersist(mutate: (state: LayoutState) => LayoutState) {
    update((state) => {
      const next = mutate(state);
      if (typeof localStorage !== 'undefined') {
        try {
          localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
        } catch {
          // Quota or privacy errors — layout persistence is best-effort
        }
      }
      return next;
    });
  }

  return {
    subscribe,
    toggleLeft() {
      updateAndPersist((s) => ({ ...s, leftCollapsed: !s.leftCollapsed }));
    },
    toggleRight() {
      updateAndPersist((s) => ({ ...s, rightCollapsed: !s.rightCollapsed }));
    },
    setRightTab(tab: RightPanelTab) {
      updateAndPersist((s) => ({ ...s, rightTab: tab, rightCollapsed: false }));
    },
    setLeftWidth(width: number) {
      updateAndPersist((s) => ({ ...s, leftWidth: clamp(width, LEFT_WIDTH_MIN, LEFT_WIDTH_MAX) }));
    },
    setRightWidth(width: number) {
      updateAndPersist((s) => ({ ...s, rightWidth: clamp(width, RIGHT_WIDTH_MIN, RIGHT_WIDTH_MAX) }));
    },
    toggleSection(key: 'sessionsCollapsed' | 'recentCollapsed' | 'agentsCollapsed') {
      updateAndPersist((s) => ({ ...s, [key]: !s[key] }));
    },
    setMaximizedTerminalId(id: string | null) {
      updateAndPersist((s) => ({ ...s, maximizedTerminalId: id }));
    },
    toggleMaximizedTerminal(id: string) {
      updateAndPersist((s) => ({
        ...s,
        maximizedTerminalId: s.maximizedTerminalId === id ? null : id,
      }));
    },
  };
}

export const layout = createLayoutStore();
