import { writable } from 'svelte/store';
import { apiUrl } from '$lib/config';

export type LayoutMode = 'focused';

interface UIState {
  focusedAgentId: string | null;
  selectedCellId: string | null;
  selectedAgentId: string | null;
  layoutMode: LayoutMode;
  cellGridCollapsed: boolean;
  terminalMaximized: boolean;
}

/**
 * The session whose navigation state writes are routed to the SQLite application_state
 * layer. Set via `ui.setPersistSession(sessionId)` when the active session changes; null
 * disables persistence (writes become no-ops).
 */
let persistSessionId: string | null = null;

/** Per-key debounce timers so a burst of mutations collapses to one POST per key. */
const persistTimers = new Map<string, ReturnType<typeof setTimeout>>();
const PERSIST_DEBOUNCE_MS = 150;

/**
 * Debounced write of a single navigation key to the backend application_state store.
 * Best-effort: failures are swallowed (persistence must never break navigation).
 */
function persistUiState(key: string, value: unknown) {
  const sessionId = persistSessionId;
  if (!sessionId) return;

  const existing = persistTimers.get(key);
  if (existing) clearTimeout(existing);

  persistTimers.set(
    key,
    setTimeout(() => {
      persistTimers.delete(key);
      fetch(apiUrl(`/api/sessions/${sessionId}/application-state`), {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ key, value }),
      }).catch(() => {
        // Navigation-state persistence is best-effort.
      });
    }, PERSIST_DEBOUNCE_MS)
  );
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
    /** Route subsequent navigation writes to this session's application_state. */
    setPersistSession(sessionId: string | null) {
      persistSessionId = sessionId;
    },
    setFocusedAgent(id: string | null) {
      update((state) => ({ ...state, focusedAgentId: id }));
      persistUiState('focusedAgentId', id);
    },
    setSelectedCell(id: string | null) {
      update((state) => ({ ...state, selectedCellId: id }));
      persistUiState('selectedCellId', id);
    },
    setSelectedAgent(id: string | null) {
      update((state) => ({ ...state, selectedAgentId: id }));
      persistUiState('selectedAgentId', id);
    },
    setLayoutMode(mode: LayoutMode) {
      update((state) => ({ ...state, layoutMode: mode }));
      persistUiState('layoutMode', mode);
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
