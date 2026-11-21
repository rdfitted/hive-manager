import { writable } from 'svelte/store';

export type OverviewTab = 'overview' | 'plan' | 'graph';
export type ActivityTab = 'activity' | 'logs' | 'timeline';
export type ChangesTab = 'changes' | 'files' | 'artifacts';
export type RightPanelTab = OverviewTab | ActivityTab | ChangesTab;

export function migrateRightTab(raw: unknown): RightPanelTab {
  switch (raw) {
    case 'activity':
    case 'chat':
      return 'activity';
    case 'changes':
    case 'review':
      return 'changes';
    case 'plan':
    case 'graph':
    case 'logs':
    case 'timeline':
    case 'files':
    case 'artifacts':
      return raw;
    default:
      return 'overview';
  }
}

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
  sessionViewMode: 'focus' | 'grid';
  /** The designed default composition this layout has adopted. */
  composition: number;
}

const STORAGE_KEY = 'hive-manager-layout';

export const LEFT_WIDTH_MIN = 200;
export const LEFT_WIDTH_MAX = 420;
export const RIGHT_WIDTH_MIN = 260;
export const RIGHT_WIDTH_MAX = 560;
export const COLLAPSED_PANEL_WIDTH = 0;
// 20px window inset plus the 56px glass rail.
export const SESSION_RAIL_WIDTH = 76;

/**
 * Bump when the designed resting view changes. A layout saved under an older
 * composition adopts the new one once; later choices persist as usual.
 * 2 (0.73.1): the stage with the session rail and a closed inspector.
 */
export const LAYOUT_COMPOSITION = 2;

const defaultLayout: LayoutState = {
  leftCollapsed: true,
  leftWidth: 250,
  rightCollapsed: true,
  rightWidth: 320,
  rightTab: 'overview',
  sessionsCollapsed: false,
  recentCollapsed: true,
  agentsCollapsed: false,
  maximizedTerminalId: null,
  sessionViewMode: 'grid',
  composition: LAYOUT_COMPOSITION,
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
    const adopt = parsed.composition !== LAYOUT_COMPOSITION;
    return {
      ...defaultLayout,
      ...parsed,
      ...(adopt ? { leftCollapsed: true, rightCollapsed: true } : {}),
      sessionViewMode: !adopt && parsed.sessionViewMode === 'focus' ? 'focus' : 'grid',
      composition: LAYOUT_COMPOSITION,
      rightTab: migrateRightTab(parsed.rightTab),
      leftWidth: clamp(parsed.leftWidth ?? defaultLayout.leftWidth, LEFT_WIDTH_MIN, LEFT_WIDTH_MAX),
      rightWidth: clamp(parsed.rightWidth ?? defaultLayout.rightWidth, RIGHT_WIDTH_MIN, RIGHT_WIDTH_MAX),
      // Maximizing is intentionally transient: reopening the app always restores the grid.
      maximizedTerminalId: null,
    };
  } catch {
    return defaultLayout;
  }
}

export function createLayoutStore() {
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
    setLeftCollapsed(collapsed: boolean) {
      updateAndPersist((s) => ({ ...s, leftCollapsed: collapsed }));
    },
    toggleRight() {
      updateAndPersist((s) => ({ ...s, rightCollapsed: !s.rightCollapsed }));
    },
    setRightTab(tab: RightPanelTab) {
      updateAndPersist((s) => ({ ...s, rightTab: tab, rightCollapsed: false }));
    },
    activateRightTab(tab: RightPanelTab) {
      updateAndPersist((s) =>
        tab === s.rightTab && !s.rightCollapsed
          ? { ...s, rightCollapsed: true }
          : { ...s, rightTab: tab, rightCollapsed: false },
      );
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
    setSessionViewMode(mode: LayoutState['sessionViewMode']) {
      updateAndPersist((s) => ({ ...s, sessionViewMode: mode }));
    },
    toggleSessionViewMode() {
      updateAndPersist((s) => ({
        ...s,
        sessionViewMode: s.sessionViewMode === 'focus' ? 'grid' : 'focus',
      }));
    },
  };
}

export const layout = createLayoutStore();
