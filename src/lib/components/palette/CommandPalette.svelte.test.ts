import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { get } from 'svelte/store';
import terminalSource from '../Terminal.svelte?raw';

vi.mock('$lib/stores/sessions', async () => {
  const { writable, derived } = await import('svelte/store');
  const store = writable<{ sessions: any[]; activeSessionId: string | null }>({ sessions: [], activeSessionId: null });
  return {
    sessions: { ...store, setActiveSession: vi.fn((id) => store.update((state) => ({ ...state, activeSessionId: id }))) },
    activeSession: derived(store, (state) => state.sessions.find((session) => session.id === state.activeSessionId) ?? null),
  };
});
vi.mock('$lib/stores/ui', async () => {
  const { writable } = await import('svelte/store');
  const store = writable({ focusedAgentId: 'worker-5' });
  return { ui: { ...store, setFocusedAgent: vi.fn((id) => store.set({ focusedAgentId: id })) } };
});
vi.mock('$lib/stores/layout', () => ({ layout: { toggleRight: vi.fn(), setRightTab: vi.fn(), toggleSessionViewMode: vi.fn() } }));
vi.mock('$lib/stores/shell', () => ({ shell: { openAddWorker: vi.fn(), requestStartAction: vi.fn() } }));
vi.mock('$app/navigation', () => ({ goto: vi.fn().mockResolvedValue(undefined) }));
import { goto } from '$app/navigation';
import { sessions } from '$lib/stores/sessions';
import { ui } from '$lib/stores/ui';
import { layout } from '$lib/stores/layout';
import { shell } from '$lib/stores/shell';
import CommandPalette from './CommandPalette.svelte';

const fixture = { id: 's', name: 'Primary', project_path: 'D:/project', agents: [
  { id: 'worker-5', config: { label: 'Frontend' } }, { id: 'worker-6', config: { label: 'Focus view' } },
] };
const setSessions = (value: unknown) => (sessions as unknown as { set: (value: unknown) => void }).set(value);
beforeEach(() => {
  setSessions({ sessions: [fixture, { ...fixture, id: 'second', name: 'Second' }], activeSessionId: 's' });
  (ui as unknown as { set: (value: unknown) => void }).set({ focusedAgentId: 'worker-5' });
});
afterEach(() => { cleanup(); document.body.innerHTML = ''; vi.clearAllMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

async function openPalette(target: EventTarget = window, options: KeyboardEventInit = { key: 'k', ctrlKey: true }) {
  const event = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...options });
  target.dispatchEvent(event);
  await waitFor(() => expect(document.querySelector('[aria-label="Command palette"]')).toBeTruthy());
  return event;
}

describe('G-palette-key', () => {
  it.each(['ctrlKey', 'metaKey'] as const)('opens on %s+K outside xterm and focuses search', async (modifier) => {
    const view = render(CommandPalette);
    const event = await openPalette(window, { key: 'k', [modifier]: true });
    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(view.getByRole('textbox', { name: 'Find command' }));
  });
  it.each(['ctrlKey', 'metaKey'] as const)('leaves %s+K inside an xterm untouched even when it bubbles', async (modifier) => {
    const view = render(CommandPalette);
    const host = document.createElement('div'); host.className = 'xterm';
    const input = document.createElement('textarea'); host.append(input); document.body.append(host);
    const event = new KeyboardEvent('keydown', { key: 'k', [modifier]: true, bubbles: true, cancelable: true });
    input.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(view.queryByRole('dialog')).toBeNull();
  });
  it.each(['ctrlKey', 'metaKey'] as const)('opens on %s+Shift+P from the page', async (modifier) => {
    render(CommandPalette);
    expect((await openPalette(window, { key: 'P', [modifier]: true, shiftKey: true })).defaultPrevented).toBe(true);
  });
  it.each(['ctrlKey', 'metaKey'] as const)('opens from real xterm on %s+Shift+P; keeps Ctrl+K as terminal data', async (modifier) => {
    vi.stubGlobal('matchMedia', vi.fn(() => ({ matches: false, addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {} })));
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
    const { Terminal } = await import('@xterm/xterm');
    const term = new Terminal();
    const host = document.createElement('div'); document.body.append(host);
    const view = render(CommandPalette);
    try {
      term.open(host);
      const prefix = terminalSource.split('term.attachCustomKeyEventHandler((event) => {')[1].split('// Ctrl+F opens the find bar')[0];
      term.attachCustomKeyEventHandler(new Function('event', `${prefix}\nreturn true;`) as (event: KeyboardEvent) => boolean);
      const data = vi.fn(); term.onData(data);
      const kill = new KeyboardEvent('keydown', { key: 'k', keyCode: 75, ctrlKey: true, bubbles: true, cancelable: true });
      term.textarea!.dispatchEvent(kill);
      expect(data).toHaveBeenCalledWith('\x0b');
      expect(kill.defaultPrevented).toBe(true);
      expect(view.queryByRole('dialog')).toBeNull();
      data.mockClear();
      await openPalette(term.textarea!, { key: 'P', keyCode: 80, [modifier]: true, shiftKey: true });
      expect(data).not.toHaveBeenCalled();
    } finally { term.dispose(); host.remove(); }
  });
  it('ignores repeated and Alt-modified palette chords', () => {
    const view = render(CommandPalette);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'k', ctrlKey: true, repeat: true }));
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'P', ctrlKey: true, shiftKey: true, altKey: true }));
    expect(view.queryByRole('dialog')).toBeNull();
  });
});

describe('command action wiring and navigation', () => {
  it.each([
    ['Sessions', '/'], ['Dashboard', '/dashboard'], ['Knowledge', '/knowledge'],
  ])('keeps %s reachable from the palette with the sidebar collapsed', async (destination, path) => {
    const view = render(CommandPalette); await openPalette();
    await fireEvent.click(view.getByRole('button', { name: `Go to ${destination}` }));
    expect(goto).toHaveBeenCalledWith(path);
    expect(view.queryByRole('dialog')).toBeNull();
  });
  it.each([
    ['New session', () => expect(shell.requestStartAction).toHaveBeenCalledWith('hive')],
    ['Add worker', () => expect(shell.openAddWorker).toHaveBeenCalledOnce()],
    ['Toggle right panel', () => expect(layout.toggleRight).toHaveBeenCalledOnce()],
    ['Open Overview', () => expect(layout.setRightTab).toHaveBeenCalledWith('overview')],
    ['Open Activity', () => expect(layout.setRightTab).toHaveBeenCalledWith('activity')],
    ['Open Changes', () => expect(layout.setRightTab).toHaveBeenCalledWith('changes')],
    ['Toggle Focus / Grid', () => expect(layout.toggleSessionViewMode).toHaveBeenCalledOnce()],
    ['Focus agent: Focus view', () => expect(ui.setFocusedAgent).toHaveBeenCalledWith('worker-6')],
    ['Switch session: Second', () => expect(sessions.setActiveSession).toHaveBeenCalledWith('second')],
  ] as const)('%s uses the existing store action', async (name, verify) => {
    const view = render(CommandPalette); await openPalette();
    await fireEvent.click(view.getByRole('button', { name }));
    verify(); expect(view.queryByRole('dialog')).toBeNull();
  });
  it('opens shortcuts through the existing Ctrl+/ window key handler', async () => {
    const seen = vi.fn((event: KeyboardEvent) => { if (event.ctrlKey && event.key === '/') return true; });
    window.addEventListener('keydown', seen);
    try {
      const view = render(CommandPalette); await openPalette(); seen.mockClear();
      await fireEvent.click(view.getByRole('button', { name: 'Open keyboard shortcuts' }));
      expect(seen).toHaveBeenCalledOnce();
      expect(seen.mock.calls[0][0].key).toBe('/'); expect(seen.mock.calls[0][0].ctrlKey).toBe(true);
    } finally { window.removeEventListener('keydown', seen); }
  });
  it('filters commands and executes the selected result with Enter', async () => {
    const view = render(CommandPalette); await openPalette();
    const search = view.getByRole('textbox', { name: 'Find command' });
    await fireEvent.input(search, { target: { value: 'open aCTivity' } });
    expect(view.queryByRole('button', { name: 'New session' })).toBeNull();
    await fireEvent.keyDown(search, { key: 'Enter' });
    expect(layout.setRightTab).toHaveBeenCalledWith('activity');
  });
  it('supports arrow navigation and an empty search result', async () => {
    const view = render(CommandPalette); await openPalette();
    const search = view.getByRole('textbox', { name: 'Find command' });
    await fireEvent.input(search, { target: { value: 'Open ' } });
    await fireEvent.keyDown(search, { key: 'ArrowDown' });
    await fireEvent.keyDown(search, { key: 'Enter' });
    expect(layout.setRightTab).toHaveBeenCalledWith('activity');
    await openPalette();
    await fireEvent.input(view.getByRole('textbox', { name: 'Find command' }), { target: { value: 'no such command' } });
    expect(view.getByText('No commands match “no such command”.')).toBeTruthy();
  });
  it('restores focus on Escape and contains Tab navigation', async () => {
    const previous = document.createElement('button'); document.body.append(previous); previous.focus();
    const view = render(CommandPalette); await openPalette();
    const close = view.getByRole('button', { name: 'Close command palette' });
    close.focus(); await fireEvent.keyDown(close, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(view.getByRole('button', { name: 'Flush Enter: Frontend' }));
    await fireEvent.keyDown(document.activeElement!, { key: 'Tab' });
    expect(document.activeElement).toBe(close);
    await fireEvent.keyDown(close, { key: 'Escape' });
    expect(view.queryByRole('dialog')).toBeNull(); expect(document.activeElement).toBe(previous);
  });
  it('contains arrows on command buttons before the page can navigate agents', async () => {
    const view = render(CommandPalette); await openPalette();
    const navigate = vi.fn((event: KeyboardEvent) => { if (event.key === 'ArrowDown' || event.key === 'ArrowUp') ui.setFocusedAgent('page-navigation'); });
    window.addEventListener('keydown', navigate);
    try {
      const button = view.getByRole('button', { name: 'New session' }); button.focus();
      await fireEvent.keyDown(button, { key: 'ArrowDown' });
      expect(navigate).not.toHaveBeenCalled();
      expect(get(ui).focusedAgentId).toBe('worker-5');
    } finally { window.removeEventListener('keydown', navigate); }
  });
  it('flushes the focused agent through HTTP and shows a sentence with its exact receipt in closed Details', async () => {
    const receipt = '{"submit_confirmed":true}';
    const fetcher = vi.fn().mockResolvedValue({ ok: true, text: async () => receipt }); vi.stubGlobal('fetch', fetcher);
    const view = render(CommandPalette); await openPalette();
    await fireEvent.click(view.getByRole('button', { name: 'Flush Enter: Frontend' }));
    await waitFor(() => expect(view.getByRole('status').textContent).toBe('Enter delivered.'));
    const details = view.getByRole('dialog').querySelector('details')!;
    expect(details.open).toBe(false);
    expect(details.querySelector('pre')?.textContent).toBe(receipt);
    expect(JSON.parse(fetcher.mock.calls[0][1].body)).toEqual({ target_agent_id: 'worker-5', message: '', submit: true });
    expect(view.getByRole('dialog')).toBeTruthy();
  });
  it.each([
    ['{"submit_confirmed":null}', "Enter sent. Delivery isn't confirmed yet."],
    ['{"submit_confirmed":false}', 'Enter may not have landed. Check the terminal.'],
    ['legacy receipt', 'Enter sent.'],
    ['', 'Enter sent.'],
  ])('G21: summarizes %s without moving focus and dismisses the raw Details too', async (receipt, sentence) => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, text: async () => receipt }));
    const view = render(CommandPalette); await openPalette();
    const flush = view.getByRole('button', { name: 'Flush Enter: Frontend' }); flush.focus();
    await fireEvent.click(flush);
    await waitFor(() => expect(view.getByRole('status').textContent, 'receipt status must be a plain sentence').toBe(sentence));
    expect(document.activeElement).toBe(flush);
    const details = view.getByRole('dialog').querySelector('details')!;
    expect(details.open).toBe(false);
    expect(details.querySelector('pre')?.textContent).toBe(receipt);
    await fireEvent.click(view.getByRole('button', { name: 'Dismiss Enter receipt' }));
    expect(view.queryByRole('status')).toBeNull();
    expect(view.getByRole('dialog').querySelector('details')).toBeNull();
  });
  it('G21: keeps receipt Details in the palette keyboard loop', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, text: async () => '{"submit_confirmed":true}' }));
    const view = render(CommandPalette); await openPalette();
    await fireEvent.click(view.getByRole('button', { name: 'Flush Enter: Frontend' }));
    await waitFor(() => expect(view.getByRole('status').textContent).toBe('Enter delivered.'));
    const close = view.getByRole('button', { name: 'Close command palette' });
    const summary = view.getByRole('dialog').querySelector('summary')!;
    close.focus(); await fireEvent.keyDown(close, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(summary);
    await fireEvent.keyDown(summary, { key: 'Tab' });
    expect(document.activeElement).toBe(close);
    const dismiss = view.getByRole('button', { name: 'Dismiss Enter receipt' });
    dismiss.focus();
    expect(await fireEvent.keyDown(dismiss, { key: 'Tab' }), 'Tab from Dismiss must reach the following Details control').toBe(true);
  });
  it('G21: keeps palette flush errors visible instead of auto-clearing them', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline fixture')));
    const view = render(CommandPalette); await openPalette();
    await fireEvent.click(view.getByRole('button', { name: 'Flush Enter: Frontend' }));
    await waitFor(() => expect(view.getByRole('alert').textContent).toContain('offline fixture'));
    vi.useFakeTimers();
    try {
      await vi.advanceTimersByTimeAsync(60000);
      expect(view.getByRole('alert').textContent).toContain('offline fixture');
      expect(view.queryByRole('button', { name: 'Dismiss Enter receipt' })).toBeNull();
    } finally { vi.useRealTimers(); }
  });
  it('hides session-specific actions without an active session or focused agent', async () => {
    setSessions({ sessions: [], activeSessionId: null });
    const view = render(CommandPalette); await openPalette();
    expect(view.queryByRole('button', { name: 'Add worker' })).toBeNull();
    expect(view.queryByRole('button', { name: /Flush Enter/ })).toBeNull();
  });
  it('does not move a pending receipt into a later palette opening', async () => {
    let resolve!: (value: unknown) => void;
    vi.stubGlobal('fetch', vi.fn(() => new Promise((done) => { resolve = done; })));
    const view = render(CommandPalette); await openPalette();
    await fireEvent.click(view.getByRole('button', { name: 'Flush Enter: Frontend' }));
    await fireEvent.keyDown(view.getByRole('textbox', { name: 'Find command' }), { key: 'Escape' });
    await openPalette();
    expect(view.getByRole('button', { name: 'Flush Enter: Frontend' }).getAttribute('aria-disabled')).toBe('true');
    resolve({ ok: true, text: async () => 'receipt from previous opening' });
    await waitFor(() => expect(view.getByRole('button', { name: 'Flush Enter: Frontend' }).hasAttribute('aria-disabled')).toBe(false));
    expect(view.queryByText('receipt from previous opening')).toBeNull();
  });
  it('keeps pending Flush focused, blocks duplicate clicks and search Enter, and contains Tab/Escape', async () => {
    let resolve!: (value: unknown) => void;
    const fetcher = vi.fn(() => new Promise((done) => { resolve = done; }));
    vi.stubGlobal('fetch', fetcher);
    const previous = document.createElement('button'); document.body.append(previous); previous.focus();
    const view = render(CommandPalette); await openPalette();
    const flush = view.getByRole('button', { name: 'Flush Enter: Frontend' });
    flush.focus(); await fireEvent.click(flush);
    expect(document.activeElement).toBe(flush);
    expect(Array.from(view.getByRole('dialog').querySelectorAll('[aria-disabled="true"]'))).toEqual([flush]);
    expect(view.getByRole('dialog').querySelectorAll('button:disabled')).toHaveLength(0);
    await fireEvent.click(flush);
    const search = view.getByRole('textbox', { name: 'Find command' });
    search.focus(); await fireEvent.input(search, { target: { value: 'Flush Enter' } });
    await fireEvent.keyDown(search, { key: 'Enter' });
    expect(fetcher).toHaveBeenCalledOnce();
    flush.focus(); await fireEvent.keyDown(flush, { key: 'Tab' });
    const close = view.getByRole('button', { name: 'Close command palette' });
    expect(document.activeElement).toBe(close);
    await fireEvent.keyDown(close, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(flush);
    await fireEvent.keyDown(flush, { key: 'Escape' });
    expect(view.queryByRole('dialog')).toBeNull(); expect(document.activeElement).toBe(previous);
    resolve({ ok: true, text: async () => 'settled after closing' });
    await openPalette();
    await waitFor(() => expect(view.getByRole('button', { name: 'Flush Enter: Frontend' }).hasAttribute('aria-disabled')).toBe(false));
    expect(view.queryByText('settled after closing')).toBeNull();
  });
  it.each(['immediately', 'after reopening'])('unrelated command runs while a flush is pending (%s)', async (when) => {
    let resolve!: (value: unknown) => void;
    const fetcher = vi.fn(() => new Promise((done) => { resolve = done; }));
    vi.stubGlobal('fetch', fetcher);
    const view = render(CommandPalette); await openPalette();
    const flush = view.getByRole('button', { name: 'Flush Enter: Frontend' });
    flush.focus(); await fireEvent.click(flush);
    if (when === 'after reopening') {
      await fireEvent.keyDown(flush, { key: 'Escape' });
      await openPalette();
    }
    expect(view.getByRole('button', { name: 'Flush Enter: Frontend' }).getAttribute('aria-disabled')).toBe('true');
    await fireEvent.click(view.getByRole('button', { name: 'Open Activity' }));
    expect(layout.setRightTab).toHaveBeenCalledWith('activity');
    expect(view.queryByRole('dialog')).toBeNull(); expect(fetcher).toHaveBeenCalledOnce();
    resolve({ ok: true, text: async () => 'settled after navigation' });
    await openPalette();
    await waitFor(() => expect(view.getByRole('button', { name: 'Flush Enter: Frontend' }).hasAttribute('aria-disabled')).toBe(false));
    expect(view.queryByText('settled after navigation')).toBeNull();
  });
  it('registers Alt+Shift+F6 without Ctrl, Meta, or repeat', () => {
    render(CommandPalette);
    const event = new KeyboardEvent('keydown', { key: 'F6', altKey: true, shiftKey: true, cancelable: true });
    window.dispatchEvent(event);
    expect(layout.toggleSessionViewMode).toHaveBeenCalledOnce(); expect(event.defaultPrevented).toBe(true);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'F6', altKey: true, shiftKey: true, ctrlKey: true }));
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'F6', altKey: true, shiftKey: true, repeat: true }));
    expect(layout.toggleSessionViewMode).toHaveBeenCalledOnce();
  });
});
