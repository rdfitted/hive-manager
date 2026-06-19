import { writable } from 'svelte/store';

/**
 * Per-session composer draft persistence (#128).
 *
 * Clones the `layout.ts` `createLayoutStore()` pattern: `loadInitial` reads from
 * localStorage with an SSR guard, and `updateAndPersist` writes back inside a try/catch so
 * a quota/privacy failure degrades to in-memory only (never throws). Each session keeps its
 * own key `hive-composer-draft-{sessionId}` so drafts are isolated. Writes are debounced so
 * a burst of keystrokes collapses to one localStorage write per session.
 */

const KEY_PREFIX = 'hive-composer-draft-';
const DEBOUNCE_MS = 400;

/** Build the per-session storage key. Exported for tests / introspection. */
export function draftKey(sessionId: string): string {
  return `${KEY_PREFIX}${sessionId}`;
}

function loadInitial(sessionId: string): string {
  if (typeof localStorage === 'undefined') return '';
  try {
    return localStorage.getItem(draftKey(sessionId)) ?? '';
  } catch {
    return '';
  }
}

function createComposerDraftStore() {
  // Holds the live draft text for whichever session is currently bound.
  const { subscribe, set } = writable<string>('');

  let boundSessionId: string | null = null;
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  /** In-memory fallback used when localStorage is unavailable (quota/privacy). */
  const memory = new Map<string, string>();

  function persist(sessionId: string, text: string) {
    memory.set(sessionId, text);
    if (typeof localStorage === 'undefined') return;
    try {
      if (text.length === 0) {
        localStorage.removeItem(draftKey(sessionId));
      } else {
        localStorage.setItem(draftKey(sessionId), text);
      }
    } catch {
      // Quota or privacy errors — draft persistence is best-effort (in-memory only).
    }
  }

  return {
    subscribe,

    /**
     * Bind the store to a session, hydrating its persisted (or in-memory) draft. Call on
     * mount and whenever the active session changes.
     */
    load(sessionId: string): string {
      if (debounceTimer) {
        clearTimeout(debounceTimer);
        debounceTimer = null;
      }
      boundSessionId = sessionId;
      const initial = memory.has(sessionId) ? memory.get(sessionId)! : loadInitial(sessionId);
      memory.set(sessionId, initial);
      set(initial);
      return initial;
    },

    /** Update the bound session's draft (debounced persist). */
    update(text: string) {
      const sessionId = boundSessionId;
      set(text);
      if (!sessionId) return;
      memory.set(sessionId, text);
      if (debounceTimer) clearTimeout(debounceTimer);
      debounceTimer = setTimeout(() => {
        debounceTimer = null;
        persist(sessionId, text);
      }, DEBOUNCE_MS);
    },

    /** Clear the bound session's draft immediately (e.g. after submit). */
    clear() {
      const sessionId = boundSessionId;
      set('');
      if (!sessionId) return;
      if (debounceTimer) {
        clearTimeout(debounceTimer);
        debounceTimer = null;
      }
      persist(sessionId, '');
    },

    /** Synchronously read a session's persisted draft without binding (tests/preview). */
    read(sessionId: string): string {
      return memory.has(sessionId) ? memory.get(sessionId)! : loadInitial(sessionId);
    },

    /** Flush any pending debounced write immediately. Mainly for tests. */
    flush() {
      if (debounceTimer && boundSessionId) {
        clearTimeout(debounceTimer);
        debounceTimer = null;
        persist(boundSessionId, memory.get(boundSessionId) ?? '');
      }
    },
  };
}

export const composerDraft = createComposerDraftStore();
