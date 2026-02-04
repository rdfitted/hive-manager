import { writable } from 'svelte/store';

export type LayoutMode = 'grid' | 'focused';

interface UIState {
  focusedAgentId: string | null;
  layoutMode: LayoutMode;
}

function createUIStore() {
  const { subscribe, set, update } = writable<UIState>({
    focusedAgentId: null,
    layoutMode: 'focused',
  });

  return {
    subscribe,
    setFocusedAgent(id: string | null) {
      update((state) => ({ ...state, focusedAgentId: id }));
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
  };
}

export const ui = createUIStore();
