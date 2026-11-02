import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { Session } from '$lib/stores/sessions';
import type { AgentEventsState } from '$lib/stores/agentEvents';

const fixture = vi.hoisted(() => ({
  terminals: [] as { buffer: string; disposed: boolean; textarea: HTMLTextAreaElement; keyHandler: (event: KeyboardEvent) => boolean }[],
  webglCreated: 0, webglDisposed: 0, focuses: 0,
  setAgentEvents: null as ((value: AgentEventsState) => void) | null,
  invoke: vi.fn(async (command: string, args: { id: string }) => command === 'get_pty_snapshot'
    ? { id: args.id, offset_start: 0, offset_end: 17, data: btoa('fixture scrollback') } : undefined),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('phosphor-svelte', () => ({
  ArrowDown: () => {}, ArrowUp: () => {}, Broom: () => {}, CaretDown: () => {}, CaretUp: () => {}, CheckSquare: () => {},
  ClipboardText: () => {}, FileText: () => {}, MagnifyingGlass: () => {}, X: () => {},
  ArrowsIn: () => {}, ArrowsOut: () => {}, Check: () => {}, Circle: () => {}, Hourglass: () => {}, Plus: () => {},
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ onDragDropEvent: vi.fn().mockResolvedValue(() => {}) }) }));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ writeText: vi.fn(), readText: vi.fn() }));
vi.mock('@xterm/xterm', () => ({ Terminal: class {
  buffer = ''; disposed = false; textarea = document.createElement('textarea');
  keyHandler = (_event: KeyboardEvent) => true;
  constructor(public options: Record<string, unknown>) { fixture.terminals.push(this); }
  open(node: HTMLElement) { this.textarea.className = 'xterm-helper-textarea'; node.append(this.textarea); }
  write(text: string) { this.buffer += text; }
  focus() { fixture.focuses++; this.textarea.focus(); }
  loadAddon() {}
  onData() { return { dispose() {} }; }
  attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean) { this.keyHandler = handler; }
  getSelection() { return ''; }
  scrollToBottom() {}
  dispose() { this.disposed = true; }
} }));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: class { fit() {} proposeDimensions() { return { cols: 80, rows: 24 }; } } }));
vi.mock('@xterm/addon-search', () => ({ SearchAddon: class { onDidChangeResults() { return { dispose() {} }; } clearDecorations() {} } }));
vi.mock('@xterm/addon-webgl', () => ({ WebglAddon: class {
  constructor() { fixture.webglCreated++; }
  onContextLoss() { return { dispose() {} }; }
  dispose() { fixture.webglDisposed++; }
} }));
vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    activeAgents: writable([]), activeSession: writable(null), sessions: writable({ sessions: [] }),
    serdeEnumVariantName: (value: unknown) => typeof value === 'string' ? value : value && typeof value === 'object' ? Object.keys(value)[0] : undefined,
  };
});
vi.mock('$lib/stores/events', async () => {
  const { writable } = await import('svelte/store');
  return { events: writable({ events: [] }) };
});
vi.mock('$lib/stores/conversations', async () => {
  const { writable } = await import('svelte/store');
  return { heartbeatStore: { subscribe: writable({ agents: {}, staleAgents: new Set() }).subscribe, loadHeartbeats: vi.fn() } };
});
vi.mock('$lib/stores/agentEvents', async () => {
  const { writable } = await import('svelte/store');
  const state = writable<AgentEventsState>();
  fixture.setAgentEvents = state.set;
  return { agentEvents: { subscribe: state.subscribe } };
});

function publishFeed(withRecord = true) {
  fixture.setAgentEvents!({ sessionId: 's', feeds: {
    claude: { agent_id: 'claude', capabilities: { can_interrupt: true, supports_approvals: false, has_terminal: true, event_source: 'transcript' } },
    codex: { agent_id: 'codex', capabilities: { can_interrupt: true, supports_approvals: false, has_terminal: true, event_source: 'none' } },
  }, histories: { claude: { events: withRecord ? [{ agent_id: 'claude', cursor: 'first', source_id: 'first', kind: 'assistant_message', ts: '2026-01-01T00:00:00Z', truncated: false, payload: { text: 'Fixture transcript' } }] : [], before: null, hasMore: false, loading: false, error: null } }, connected: true, error: null });
}

import TerminalGrid from '../TerminalGrid.svelte';
import { layout } from '$lib/stores/layout';
const { readFileSync } = await vi.importActual<{ readFileSync(path: string, encoding: 'utf8'): string }>('node:fs');

beforeEach(() => {
  fixture.terminals.length = 0; fixture.webglCreated = 0; fixture.webglDisposed = 0; fixture.focuses = 0;
  localStorage.clear(); layout.setMaximizedTerminalId(null);
  publishFeed();
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  // Real terminal palette resolution requires concrete hex values in jsdom.
  const source = readFileSync('src/lib/components/Terminal.svelte', 'utf8');
  for (const [, token] of source.matchAll(/'(--[\w-]+)'/g)) document.documentElement.style.setProperty(token, '#aaaaaa');
});
afterEach(async () => {
  // Terminal schedules its first resize after async listener/snapshot setup. Let that
  // frame complete before jsdom removes its DOM, just as a mounted desktop pane does.
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  cleanup(); document.documentElement.removeAttribute('style');
  document.head.querySelectorAll('style[data-chat-test]').forEach((node) => node.remove()); vi.unstubAllGlobals(); vi.clearAllMocks();
});

const session: Session = {
  id: 's', session_type: { Hive: { worker_count: 1 } }, state: 'Running', project_path: 'D:/fixture', worktree_path: 'D:/fixture/cell', created_at: '2026-01-01T00:00:00Z',
  agents: ['claude', 'codex'].map((id) => ({ id, role: 'Queen', parent_id: null, status: 'Running', config: { cli: id, flags: [], label: id === 'claude' ? 'Claude agent' : 'Codex PTY' } })),
};

describe('attention across pane modes (real Terminal, renderer doubles)', () => {
  it('CR-4: owns one pending Flush across Grid and Focus and retains the warning', async () => {
    layout.setSessionViewMode('grid');
    let resolve!: (response: unknown) => void;
    const fetcher = vi.fn(() => new Promise((done) => { resolve = done; }));
    vi.stubGlobal('fetch', fetcher);
    const waiting = { ...session, agents: session.agents.map((agent) => agent.id === 'claude'
      ? { ...agent, status: { WaitingForInput: 'injected_delivery_failed:fixture' } } : agent) };
    const view = render(TerminalGrid, { session: waiting, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    await fireEvent.click(view.getByRole('button', { name: 'Flush Enter' }));
    layout.setSessionViewMode('focus'); await tick();
    const pending = view.getByRole('button', { name: /^Flushing/ }) as HTMLButtonElement;
    expect(pending.disabled).toBe(true);
    await fireEvent.click(pending);
    expect(fetcher).toHaveBeenCalledOnce();
    resolve({ ok: true, text: async () => '{"submit_confirmed":false,"target":"claude"}' });
    await waitFor(() => expect(view.getByText('Enter may not have landed. Check the terminal.')).toBeTruthy());
    layout.setSessionViewMode('grid'); await tick();
    const notice = view.getByRole('region', { name: 'Agent needs attention' });
    expect(within(notice).getByText('Enter may not have landed. Check the terminal.')).toBeTruthy();
    const details = within(notice).getByText('Details');
    await fireEvent.click(details);
    expect(notice.querySelector('pre')?.textContent).toContain('"target":"claude"');
    const other = view.getByText('Codex PTY').closest('.terminal-item')!;
    expect(other.textContent).not.toContain('Enter may not have landed.');
    expect(other.querySelector('pre')).toBeNull();
    expect(fetcher).toHaveBeenCalledOnce();
    expect(fixture.terminals).toEqual(original);
    expect(original.every((term) => !term.disposed)).toBe(true);
    expect(fixture.focuses).toBe(0);
  });
  it.each(['chat', 'terminal'] as const)('CR-5: %s has no empty overlay and preserves Terminal and composer through attention changes', async (mode) => {
    layout.setSessionViewMode('grid');
    const view = render(TerminalGrid, { session, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    if (mode === 'terminal') await fireEvent.click(view.getByRole('button', { name: 'Terminal' }));
    const pane = view.getByText('Claude agent').closest('.terminal-item')!;
    const content = pane.querySelector('.terminal-container')!;
    const original = [...fixture.terminals];
    const composer = pane.querySelector('.chat-composer [role="textbox"]');
    expect(composer).toBeTruthy();
    expect(content.querySelector('.agent-info'), 'no opaque wrapper or padding without attention').toBeNull();
    expect(pane.querySelector('.terminal-header .activity-line'), 'activity belongs in the existing header').toBeTruthy();
    expect(content.querySelector('.activity-line')).toBeNull();
    const waiting = { ...session, agents: session.agents.map((agent) => agent.id === 'claude'
      ? { ...agent, status: { WaitingForInput: 'injected_delivery_failed:fixture' } } : agent) };
    await view.rerender({ session: waiting });
    const warning = within(pane as HTMLElement).getByRole('region', { name: 'Agent needs attention' });
    expect(warning.closest('.agent-info')).toBeTruthy();
    expect(within(warning).getByRole('button', { name: 'Open terminal' })).toBeTruthy();
    for (const next of ['focus', 'grid'] as const) {
      layout.setSessionViewMode(next); await tick();
      expect(within(pane as HTMLElement).getByRole('region', { name: 'Agent needs attention' })).toBeTruthy();
      expect(pane.querySelector('.chat-composer [role="textbox"]')).toBe(composer);
      expect(fixture.terminals).toHaveLength(original.length);
      original.forEach((term, index) => expect(fixture.terminals[index]).toBe(term));
      expect(original.every((term) => !term.disposed)).toBe(true);
    }
    await view.rerender({ session });
    expect(content.querySelector('.agent-info')).toBeNull();
    expect(within(pane as HTMLElement).queryByRole('region', { name: 'Agent needs attention' })).toBeNull();
    expect(pane.querySelector('.chat-composer [role="textbox"]')).toBe(composer);
    original.forEach((term, index) => expect(fixture.terminals[index]).toBe(term));
    expect(original.every((term) => !term.disposed)).toBe(true);
    expect(fixture.focuses).toBe(0);
  });

});
