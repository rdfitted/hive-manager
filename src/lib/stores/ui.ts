import { writable } from 'svelte/store';

export type LayoutMode = 'grid' | 'focused';

interface UIState {
  focusedAgentId: string | null;
  selectedCellId: string | null;
  selectedAgentId: string | null;
  layoutMode: LayoutMode;
  cellGridCollapsed: boolean;
  terminalMaximized: boolean;
}

function createUIStore() {
  const { subscribe, set, update } = writable<UIState>({
    focusedAgentId: null,
    selectedCellId: null,
    selectedAgentId: null,
    layoutMode: 'focused',
    cellGridCollapsed: false,
    terminalMaximized: false,
  });

  return {
    subscribe,
    setFocusedAgent(id: string | null) {
      update((state) => ({ ...state, focusedAgentId: id }));
    },
    setSelectedCell(id: string | null) {
      update((state) => ({ ...state, selectedCellId: id }));
    },
    setSelectedAgent(id: string | null) {
      update((state) => ({ ...state, selectedAgentId: id }));
    },
    setLayoutMode(mode: LayoutMode) {
      update((state) => ({ ...state, layoutMode: mode }));
    },
    toggleLayoutMode() {
      update((state) => ({
        ...state,
        layoutMode: state.layoutMode === 'grid' ? 'focused' : 'grid',
      }));
    },
    setCellGridCollapsed(collapsed: boolean) {
      update((state) => ({ ...state, cellGridCollapsed: collapsed }));
    },
    setTerminalMaximized(maximized: boolean) {
      update((state) => ({ ...state, terminalMaximized: maximized }));
    },
  };
}

export const ui = createUIStore();
