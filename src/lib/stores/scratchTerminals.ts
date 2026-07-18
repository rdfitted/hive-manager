import { writable } from 'svelte/store';

export type ScratchShell = 'powershell' | 'cmd';

export interface ScratchTerminalPane {
  kind: 'scratch';
  id: string;
  sessionId: string;
  title: string;
  cwd: string;
  shell: ScratchShell;
  createdAt: string;
}

interface ScratchTerminalState {
  panesBySession: Record<string, ScratchTerminalPane[]>;
  focusedBySession: Record<string, string | null>;
}

const initialState: ScratchTerminalState = {
  panesBySession: {},
  focusedBySession: {},
};

function scratchId(sessionId: string): string {
  const suffix = globalThis.crypto?.randomUUID?.()
    ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  return `scratch:${sessionId}:${suffix}`;
}

export function shellCommand(shell: ScratchShell): { command: string; args: string[] } {
  return shell === 'powershell'
    ? { command: 'powershell.exe', args: ['-NoLogo'] }
    : { command: 'cmd.exe', args: [] };
}

function createScratchTerminalStore() {
  const { subscribe, update } = writable<ScratchTerminalState>(initialState);

  return {
    subscribe,
    add(sessionId: string, cwd: string, shell: ScratchShell): ScratchTerminalPane {
      const pane: ScratchTerminalPane = {
        kind: 'scratch',
        id: scratchId(sessionId),
        sessionId,
        title: shell === 'powershell' ? 'PowerShell' : 'Command Prompt',
        cwd,
        shell,
        createdAt: new Date().toISOString(),
      };

      update((state) => ({
        panesBySession: {
          ...state.panesBySession,
          [sessionId]: [...(state.panesBySession[sessionId] ?? []), pane],
        },
        focusedBySession: {
          ...state.focusedBySession,
          [sessionId]: pane.id,
        },
      }));

      return pane;
    },
    remove(sessionId: string, id: string) {
      update((state) => {
        const remaining = (state.panesBySession[sessionId] ?? []).filter((pane) => pane.id !== id);
        return {
          panesBySession: {
            ...state.panesBySession,
            [sessionId]: remaining,
          },
          focusedBySession: {
            ...state.focusedBySession,
            [sessionId]: state.focusedBySession[sessionId] === id ? null : state.focusedBySession[sessionId] ?? null,
          },
        };
      });
    },
    focus(sessionId: string, id: string | null) {
      update((state) => ({
        ...state,
        focusedBySession: {
          ...state.focusedBySession,
          [sessionId]: id,
        },
      }));
    },
    clearSession(sessionId: string) {
      update((state) => {
        const panesBySession = { ...state.panesBySession };
        const focusedBySession = { ...state.focusedBySession };
        delete panesBySession[sessionId];
        delete focusedBySession[sessionId];
        return { panesBySession, focusedBySession };
      });
    },
  };
}

export const scratchTerminals = createScratchTerminalStore();
