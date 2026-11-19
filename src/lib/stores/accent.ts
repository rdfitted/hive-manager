import { get, writable } from 'svelte/store';

export type Accent = 'honey' | 'sky';

const STORAGE_KEY = 'hive-manager-accent';

function loadInitial(): Accent {
  try {
    return localStorage.getItem(STORAGE_KEY) === 'sky' ? 'sky' : 'honey';
  } catch {
    return 'honey';
  }
}

function applyAccent(value: Accent) {
  if (typeof document !== 'undefined') document.documentElement.dataset.accent = value;
}

export function createAccentStore() {
  const initial = loadInitial();
  const state = writable<Accent>(initial);
  applyAccent(initial);

  function set(value: Accent) {
    applyAccent(value);
    try {
      localStorage.setItem(STORAGE_KEY, value);
    } catch {
      // The current selection still applies when storage is unavailable.
    }
    state.set(value);
  }

  return {
    subscribe: state.subscribe,
    set,
    toggle: () => set(get(state) === 'honey' ? 'sky' : 'honey'),
  };
}

export const accent = createAccentStore();
