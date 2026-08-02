import { writable } from 'svelte/store';

export type ShellStartActionName = 'hive' | 'fusion' | 'debate' | 'recent';

export interface ShellStartAction {
  id: number;
  action: ShellStartActionName;
}

export interface ShellState {
  addWorkerOpen: boolean;
  startAction: ShellStartAction | null;
}

export function createShellStore() {
  const { subscribe, update } = writable<ShellState>({
    addWorkerOpen: false,
    startAction: null,
  });
  let startActionId = 0;

  return {
    subscribe,
    openAddWorker() {
      update((state) => ({ ...state, addWorkerOpen: true }));
    },
    closeAddWorker() {
      update((state) => ({ ...state, addWorkerOpen: false }));
    },
    requestStartAction(action: ShellStartActionName) {
      update((state) => ({
        ...state,
        startAction: { id: ++startActionId, action },
      }));
    },
  };
}

export const shell = createShellStore();
