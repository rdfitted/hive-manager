import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { SessionFileContent, SessionFileEntry } from '$lib/stores/sessionFiles';

interface TestSessionFilesState {
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

const storeMocks = vi.hoisted(() => ({
  setActiveSession: undefined as
    | ((session: { id: string } | null) => void)
    | undefined,
  setSessionFilesState: undefined as
    | ((state: TestSessionFilesState) => void)
    | undefined,
  setSessionId: vi.fn(),
  loadFiles: vi.fn().mockResolvedValue(true),
  pollFiles: vi.fn().mockResolvedValue(undefined),
  refresh: vi.fn().mockResolvedValue(undefined),
  selectFile: vi.fn().mockResolvedValue(undefined),
  clearError: vi.fn(),
}));

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  const activeSession = writable<{ id: string } | null>({ id: 'session-a' });
  storeMocks.setActiveSession = activeSession.set;
  return { activeSession };
});

vi.mock('$lib/stores/sessionFiles', async () => {
  const { writable } = await import('svelte/store');
  const state = writable<TestSessionFilesState>({
    sessionId: 'session-a',
    entries: [],
    selectedPath: null,
    content: null,
    loading: false,
    refreshing: false,
    contentLoading: false,
    error: null,
    contentError: null,
  });
  storeMocks.setSessionFilesState = state.set;

  return {
    SESSION_FILES_POLL_INTERVAL: 5000,
    sessionFilesStore: {
      subscribe: state.subscribe,
      setSessionId: storeMocks.setSessionId,
      loadFiles: storeMocks.loadFiles,
      pollFiles: storeMocks.pollFiles,
      refresh: storeMocks.refresh,
      selectFile: storeMocks.selectFile,
      clearError: storeMocks.clearError,
    },
  };
});

import SessionFilesView from './SessionFilesView.svelte';

function fileEntry(path: string): SessionFileEntry {
  return {
    path,
    name: path.split('/').at(-1) ?? path,
    is_dir: false,
    size: 10,
    modified: '2026-07-18T00:00:00Z',
  };
}

function directoryEntry(path: string): SessionFileEntry {
  return {
    ...fileEntry(path),
    is_dir: true,
    size: 0,
  };
}

function stateWithEntries(
  entries: SessionFileEntry[],
  overrides: Partial<TestSessionFilesState> = {},
): TestSessionFilesState {
  return {
    sessionId: 'session-a',
    entries,
    selectedPath: null,
    content: null,
    loading: false,
    refreshing: false,
    contentLoading: false,
    error: null,
    contentError: null,
    ...overrides,
  };
}

function setSessionFilesState(state: TestSessionFilesState): void {
  if (!storeMocks.setSessionFilesState) throw new Error('Session files mock was not initialized');
  storeMocks.setSessionFilesState(state);
}

function setActiveSession(session: { id: string } | null): void {
  if (!storeMocks.setActiveSession) throw new Error('Active session mock was not initialized');
  storeMocks.setActiveSession(session);
}

function treeItems(container: HTMLElement): HTMLButtonElement[] {
  return Array.from(container.querySelectorAll<HTMLButtonElement>('[role="treeitem"]'));
}

function entryName(row: Element): string {
  return row.querySelector('.entry-name')?.textContent ?? '';
}

function rowNamed(container: HTMLElement, name: string): HTMLButtonElement | undefined {
  return treeItems(container).find((row) => entryName(row) === name);
}

beforeEach(() => {
  vi.clearAllMocks();
  setActiveSession({ id: 'session-a' });
  setSessionFilesState(stateWithEntries([]));
});

afterEach(() => {
  cleanup();
});

describe('SessionFilesView file window', () => {
  it('keeps every page at 200 treeitems, preserves backend order, and reaches later entries', async () => {
    const entries = [
      fileEntry('file10.txt'),
      fileEntry('file2.txt'),
      ...Array.from({ length: 1003 }, (_, index) =>
        fileEntry(`later-${index.toString().padStart(4, '0')}.txt`),
      ),
    ];
    setSessionFilesState(stateWithEntries(entries));

    const { container, getByRole, getByText } = render(SessionFilesView);
    await tick();

    let rows = treeItems(container);
    expect(rows).toHaveLength(200);
    expect(entryName(rows[0])).toBe('file10.txt');
    expect(entryName(rows[1])).toBe('file2.txt');
    expect(getByText('Showing 1–200 of 1005')).toBeTruthy();

    const previous = getByRole('button', { name: 'Previous files' }) as HTMLButtonElement;
    const next = getByRole('button', { name: 'Next files' }) as HTMLButtonElement;
    expect(previous.disabled).toBe(true);
    expect(next.getAttribute('aria-controls')).toBe('session-files-tree');

    for (let page = 0; page < 5; page += 1) {
      await fireEvent.click(next);
      rows = treeItems(container);
      expect(rows.length).toBeLessThanOrEqual(200);
    }

    expect(rows).toHaveLength(5);
    expect(entryName(rows.at(-1)!)).toBe('later-1002.txt');
    expect(next.disabled).toBe(true);
    expect(previous.disabled).toBe(false);

    await fireEvent.click(previous);
    expect(treeItems(container)).toHaveLength(200);
  });

  it('preserves collapse behavior and resets collapse and window state on session changes', async () => {
    const entries = [
      directoryEntry('folder'),
      ...Array.from({ length: 250 }, (_, index) =>
        fileEntry(`folder/child-${index.toString().padStart(3, '0')}.txt`),
      ),
      ...Array.from({ length: 100 }, (_, index) =>
        fileEntry(`root-${index.toString().padStart(3, '0')}.txt`),
      ),
    ];
    setSessionFilesState(stateWithEntries(entries));

    const { container, getByRole, queryByRole } = render(SessionFilesView);
    await tick();

    const folder = rowNamed(container, 'folder');
    expect(folder?.getAttribute('aria-expanded')).toBe('true');
    await fireEvent.click(folder!);

    expect(folder?.getAttribute('aria-expanded')).toBe('false');
    expect(rowNamed(container, 'child-000.txt')).toBeUndefined();
    expect(rowNamed(container, 'root-000.txt')).toBeTruthy();
    expect(treeItems(container)).toHaveLength(101);
    expect(queryByRole('button', { name: 'Next files' })).toBeNull();

    setSessionFilesState(stateWithEntries(entries, { sessionId: 'session-b' }));
    setActiveSession({ id: 'session-b' });
    await tick();

    expect(rowNamed(container, 'child-000.txt')).toBeTruthy();
    expect(treeItems(container)).toHaveLength(200);

    await fireEvent.click(getByRole('button', { name: 'Next files' }));
    expect(entryName(treeItems(container)[0])).not.toBe('folder');

    setSessionFilesState(stateWithEntries(entries, { sessionId: 'session-c' }));
    setActiveSession({ id: 'session-c' });
    await tick();

    expect(entryName(treeItems(container)[0])).toBe('folder');
    expect(storeMocks.setSessionId).toHaveBeenLastCalledWith('session-c');
  });

  it('retains selection and keyed DOM identity across unchanged polling and clamps after shrink', async () => {
    const entries = Array.from({ length: 450 }, (_, index) =>
      fileEntry(`file-${index.toString().padStart(4, '0')}.txt`),
    );
    let state = stateWithEntries(entries);
    setSessionFilesState(state);

    const { container, getByRole, getByText } = render(SessionFilesView);
    await tick();

    const next = getByRole('button', { name: 'Next files' });
    await fireEvent.click(next);
    await fireEvent.click(next);

    const targetPath = 'file-0400.txt';
    const target = rowNamed(container, targetPath);
    expect(target).toBeTruthy();
    await fireEvent.click(target!);
    expect(storeMocks.selectFile).toHaveBeenCalledWith(targetPath);

    state = {
      ...state,
      selectedPath: targetPath,
      content: { path: targetPath, content: 'selected', size: 10 },
    };
    setSessionFilesState(state);
    await tick();

    const selectedBeforePoll = rowNamed(container, targetPath);
    expect(selectedBeforePoll?.classList.contains('selected')).toBe(true);

    state = { ...state, entries: state.entries.map((entry) => ({ ...entry })) };
    setSessionFilesState(state);
    await tick();

    const selectedAfterPoll = rowNamed(container, targetPath);
    expect(selectedAfterPoll).toBe(selectedBeforePoll);
    expect(selectedAfterPoll?.classList.contains('selected')).toBe(true);
    expect(treeItems(container)).toHaveLength(50);

    setSessionFilesState(
      stateWithEntries(entries.slice(0, 250), { sessionId: state.sessionId }),
    );
    await tick();
    await tick();

    expect(treeItems(container)).toHaveLength(50);
    expect(entryName(treeItems(container)[0])).toBe('file-0200.txt');
    expect(getByText('Showing 201–250 of 250')).toBeTruthy();
  });
});
