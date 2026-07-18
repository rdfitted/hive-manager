import { writable } from 'svelte/store';
import { apiUrl } from '$lib/config';

export const SESSION_FILES_POLL_INTERVAL = 5000;

export interface SessionFileEntry {
  path: string;
  name: string;
  is_dir: boolean;
  size: number;
  modified: string | number | null;
}

export interface SessionFileContent {
  path: string;
  content: string;
  size: number;
}

interface SessionFilesState {
  sessionId: string | null;
  entries: SessionFileEntry[];
  selectedPath: string | null;
  content: SessionFileContent | null;
  loading: boolean;
  refreshing: boolean;
  contentLoading: boolean;
  error: string | null;
  contentError: string | null;
}

interface LoadOptions {
  silent?: boolean;
}

function initialState(sessionId: string | null = null): SessionFilesState {
  return {
    sessionId,
    entries: [],
    selectedPath: null,
    content: null,
    loading: false,
    refreshing: false,
    contentLoading: false,
    error: null,
    contentError: null,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function parseEntries(payload: unknown): SessionFileEntry[] {
  let values: unknown;
  if (Array.isArray(payload)) {
    values = payload;
  } else if (isRecord(payload) && Array.isArray(payload.entries)) {
    values = payload.entries;
  } else if (isRecord(payload) && Array.isArray(payload.files)) {
    values = payload.files;
  }

  if (!Array.isArray(values)) {
    throw new Error('The session files response was not a file list.');
  }

  return values.map((value) => {
    if (
      !isRecord(value) ||
      typeof value.path !== 'string' ||
      typeof value.name !== 'string' ||
      typeof value.is_dir !== 'boolean' ||
      typeof value.size !== 'number'
    ) {
      throw new Error('The session files response contained an invalid entry.');
    }

    const modified = value.modified;
    if (modified !== null && typeof modified !== 'string' && typeof modified !== 'number') {
      throw new Error('The session files response contained an invalid timestamp.');
    }

    return {
      path: value.path,
      name: value.name,
      is_dir: value.is_dir,
      size: value.size,
      modified,
    };
  });
}

function parseContent(payload: unknown): SessionFileContent {
  if (
    !isRecord(payload) ||
    typeof payload.path !== 'string' ||
    typeof payload.content !== 'string' ||
    typeof payload.size !== 'number'
  ) {
    throw new Error('The session file response was invalid.');
  }

  return {
    path: payload.path,
    content: payload.content,
    size: payload.size,
  };
}

function entriesMatch(left: SessionFileEntry, right: SessionFileEntry): boolean {
  return (
    left.path === right.path &&
    left.name === right.name &&
    left.is_dir === right.is_dir &&
    left.size === right.size &&
    left.modified === right.modified
  );
}

/**
 * The list endpoint is a snapshot. Reuse unchanged entries for stable rendering,
 * while still dropping paths that disappeared since the previous poll.
 */
function mergeEntries(
  existing: SessionFileEntry[],
  incoming: SessionFileEntry[],
): SessionFileEntry[] {
  const existingByPath = new Map(existing.map((entry) => [entry.path, entry]));
  return incoming.map((entry) => {
    const previous = existingByPath.get(entry.path);
    return previous && entriesMatch(previous, entry) ? previous : entry;
  });
}

async function responseError(response: Response, fallback: string): Promise<Error> {
  let detail = '';
  try {
    const body = await response.text();
    if (body) {
      try {
        const parsed: unknown = JSON.parse(body);
        if (typeof parsed === 'string') {
          detail = parsed;
        } else if (isRecord(parsed)) {
          const candidate = parsed.error ?? parsed.message;
          if (typeof candidate === 'string') detail = candidate;
        }
      } catch {
        detail = body;
      }
    }
  } catch {
    // The status code below still provides a useful error when the body is unreadable.
  }

  return new Error(detail || `${fallback} (HTTP ${response.status})`);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function createSessionFilesStore() {
  const { subscribe, set, update } = writable<SessionFilesState>(initialState());
  let activeListRequest = 0;
  let activeContentRequest = 0;

  function getState(): SessionFilesState {
    let current = initialState();
    subscribe((state) => (current = state))();
    return current;
  }

  function setSessionId(sessionId: string | null): void {
    if (getState().sessionId === sessionId) return;
    activeListRequest += 1;
    activeContentRequest += 1;
    set(initialState(sessionId));
  }

  async function loadFiles(sessionId: string, options: LoadOptions = {}): Promise<boolean> {
    if (getState().sessionId !== sessionId) setSessionId(sessionId);

    const requestToken = ++activeListRequest;
    const silent = options.silent === true;
    update((state) => ({
      ...state,
      loading: !silent && state.entries.length === 0,
      refreshing: !silent && state.entries.length > 0,
      error: silent ? state.error : null,
    }));

    try {
      const response = await fetch(
        apiUrl(`/api/sessions/${encodeURIComponent(sessionId)}/files`),
      );
      if (!response.ok) {
        throw await responseError(response, 'Unable to load session files');
      }

      const incoming = parseEntries(await response.json());
      update((state) => {
        if (requestToken !== activeListRequest || state.sessionId !== sessionId) return state;

        const entries = mergeEntries(state.entries, incoming);
        const selectionStillExists = state.selectedPath
          ? entries.some((entry) => entry.path === state.selectedPath && !entry.is_dir)
          : false;

        return {
          ...state,
          entries,
          selectedPath: selectionStillExists ? state.selectedPath : null,
          content: selectionStillExists ? state.content : null,
          contentLoading: selectionStillExists ? state.contentLoading : false,
          contentError: selectionStillExists ? state.contentError : null,
          loading: false,
          refreshing: false,
          error: null,
        };
      });
      return requestToken === activeListRequest && getState().sessionId === sessionId;
    } catch (error) {
      update((state) => {
        if (requestToken !== activeListRequest || state.sessionId !== sessionId) return state;
        return {
          ...state,
          loading: false,
          refreshing: false,
          error: errorMessage(error),
        };
      });
      return false;
    }
  }

  async function loadContent(
    sessionId: string,
    path: string,
    options: LoadOptions = {},
  ): Promise<boolean> {
    if (getState().sessionId !== sessionId) return false;

    const requestToken = ++activeContentRequest;
    const silent = options.silent === true;
    update((state) => ({
      ...state,
      selectedPath: path,
      content: state.content?.path === path ? state.content : null,
      contentLoading: !silent,
      contentError: null,
    }));

    try {
      const query = new URLSearchParams({ path });
      const response = await fetch(
        apiUrl(
          `/api/sessions/${encodeURIComponent(sessionId)}/files/content?${query.toString()}`,
        ),
      );
      if (!response.ok) {
        throw await responseError(response, 'Unable to read this file');
      }

      const content = parseContent(await response.json());
      update((state) => {
        if (
          requestToken !== activeContentRequest ||
          state.sessionId !== sessionId ||
          state.selectedPath !== path
        ) {
          return state;
        }

        return {
          ...state,
          content,
          contentLoading: false,
          contentError: null,
        };
      });
      return requestToken === activeContentRequest && getState().selectedPath === path;
    } catch (error) {
      update((state) => {
        if (
          requestToken !== activeContentRequest ||
          state.sessionId !== sessionId ||
          state.selectedPath !== path
        ) {
          return state;
        }

        return {
          ...state,
          content: null,
          contentLoading: false,
          contentError: errorMessage(error),
        };
      });
      return false;
    }
  }

  function entryVersion(entries: SessionFileEntry[], path: string | null): string | null {
    if (!path) return null;
    const entry = entries.find((candidate) => candidate.path === path);
    return entry ? `${entry.size}|${entry.modified ?? ''}` : null;
  }

  return {
    subscribe,
    setSessionId,

    loadFiles,

    async selectFile(path: string): Promise<void> {
      const state = getState();
      if (!state.sessionId) return;
      const entry = state.entries.find((candidate) => candidate.path === path);
      if (!entry || entry.is_dir) return;
      await loadContent(state.sessionId, path);
    },

    async refresh(): Promise<void> {
      const before = getState();
      if (!before.sessionId) return;
      const loaded = await loadFiles(before.sessionId);
      const after = getState();
      if (loaded && after.sessionId === before.sessionId && after.selectedPath) {
        await loadContent(after.sessionId, after.selectedPath);
      }
    },

    /** Refresh the snapshot and re-read selected content only when its metadata changed. */
    async pollFiles(): Promise<void> {
      const before = getState();
      if (!before.sessionId || before.loading || before.refreshing) return;
      const previousVersion = entryVersion(before.entries, before.selectedPath);
      const loaded = await loadFiles(before.sessionId, { silent: true });
      if (!loaded) return;

      const after = getState();
      if (
        after.sessionId === before.sessionId &&
        after.selectedPath &&
        previousVersion !== entryVersion(after.entries, after.selectedPath)
      ) {
        await loadContent(after.sessionId, after.selectedPath, { silent: true });
      }
    },

    clearError(): void {
      update((state) => ({ ...state, error: null }));
    },

    clearContentError(): void {
      update((state) => ({ ...state, contentError: null }));
    },
  };
}

export const sessionFilesStore = createSessionFilesStore();
