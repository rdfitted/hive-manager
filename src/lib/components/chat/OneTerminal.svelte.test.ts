import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { AgentEventsState } from '$lib/stores/agentEvents';
import type { Session } from '$lib/stores/sessions';
import { terminalLifecycle } from '../session/testing/terminalLifecycle';
import TerminalGrid from '../TerminalGrid.svelte';
import { layout } from '$lib/stores/layout';

const { readFileSync } = await vi.importActual<{ readFileSync(path: string, encoding: 'utf8'): string }>('node:fs');

const fixture = vi.hoisted(() => ({
  screens: new Map<HTMLTextAreaElement, { scrollback: string }>(),
  writeToPty: vi.fn<(id: string, data: string) => void>(),
  setAgentEvents: (_data: AgentEventsState) => {},
  invoke: vi.fn(async (command: string, args: {
    id?: string;
    data?: string;
    request?: { session_id: string; target_agent_id: string; message: string };
  }) => {
    if (command === 'get_pty_snapshot' && args.id) {
      const scrollback = 'replayed output for ' + args.id;
      return { id: args.id, offset_start: 0, offset_end: scrollback.length, data: btoa(scrollback) };
    }
    if (command === 'write_to_pty' && args.id && args.data) fixture.writeToPty(args.id, args.data);
    // At the backend boundary, operator_inject delivers composer text to the selected PTY.
    if (command === 'operator_inject' && args.request) {
      fixture.writeToPty(args.request.target_agent_id, args.request.message);
    }
    if (command === 'get_coordination_log') return [];
  }),
}));

vi.mock('../Terminal.svelte', async (importOriginal) => {
  const { default: Terminal } = await importOriginal<typeof import('../Terminal.svelte')>();
  const { default: Harness, useTerminal } = await import('./OneTerminalTestHarness.svelte');
  useTerminal(Terminal);
  return { default: Harness };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ onDragDropEvent: vi.fn().mockResolvedValue(() => {}) }) }));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ writeText: vi.fn(), readText: vi.fn() }));
vi.mock('phosphor-svelte', () => ({
  ArrowsIn: () => {}, ArrowsOut: () => {}, ArrowDown: () => {}, ArrowUp: () => {}, ArrowUpRight: () => {},
  Broom: () => {}, CaretDown: () => {}, CaretUp: () => {}, ChatCircle: () => {}, Check: () => {},
  CheckSquare: () => {}, Circle: () => {}, ClipboardText: () => {}, FileText: () => {}, Hourglass: () => {},
  ListBullets: () => {}, MagnifyingGlass: () => {}, Plus: () => {}, SquareSplitVertical: () => {},
  TerminalWindow: () => {}, X: () => {},
}));
vi.mock('@xterm/xterm', () => ({ Terminal: class {
  scrollback = '';
  textarea = document.createElement('textarea');
  keyHandler = (_event: KeyboardEvent) => true;
  constructor(public options: Record<string, unknown>) { fixture.screens.set(this.textarea, this); }
  open(node: HTMLElement) { this.textarea.className = 'xterm-helper-textarea'; node.append(this.textarea); }
  get buffer() {
    return { active: { baseY: 0, cursorY: 0, getLine: () => ({ translateToString: () => this.scrollback }) } };
  }
  write(text: string, done?: () => void) { this.scrollback += text; done?.(); }
  focus() { this.textarea.focus(); }
  loadAddon() {}
  onData(handler: (data: string) => void) {
    const forward = (event: KeyboardEvent) => {
      if (event.key.length === 1 && this.keyHandler(event)) handler(event.key);
    };
    this.textarea.addEventListener('keydown', forward);
    return { dispose: () => this.textarea.removeEventListener('keydown', forward) };
  }
  attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean) { this.keyHandler = handler; }
  getSelection() { return ''; }
  scrollToBottom() {}
  dispose() { fixture.screens.delete(this.textarea); }
} }));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: class { fit() {} proposeDimensions() { return { cols: 80, rows: 24 }; } } }));
vi.mock('@xterm/addon-search', () => ({ SearchAddon: class { onDidChangeResults() { return { dispose() {} }; } clearDecorations() {} } }));
vi.mock('@xterm/addon-webgl', () => ({ WebglAddon: class { onContextLoss() { return { dispose() {} }; } dispose() {} } }));
vi.mock('$lib/stores/sessions', async (importOriginal) => {
  const { writable } = await import('svelte/store');
  const actual = await importOriginal<typeof import('$lib/stores/sessions')>();
  return { ...actual, activeAgents: writable([]), activeSession: writable(null), sessions: writable({ sessions: [] }) };
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
vi.mock('$lib/stores/pendingContext', async (importOriginal) => {
  const { pendingContext } = await importOriginal<typeof import('$lib/stores/pendingContext')>();
  return { pendingContext: { ...pendingContext, consume: vi.fn().mockResolvedValue(null) } };
});

const session: Session = {
  id: 'one-terminal-session', session_type: { Hive: { worker_count: 1 } }, state: 'Running',
  project_path: 'D:/fixture', worktree_path: 'D:/fixture/cell', created_at: '2026-01-01T00:00:00Z',
  agents: [
    { id: 'queen', role: 'Queen', parent_id: null, status: 'Running', config: { cli: 'claude', flags: [], label: 'Queen' } },
    { id: 'worker', role: { Worker: { index: 1, parent: null } }, parent_id: 'queen', status: 'Running', config: { cli: 'claude', flags: [], label: 'Worker' } },
  ],
};

beforeEach(() => {
  localStorage.clear();
  layout.setMaximizedTerminalId(null);
  layout.setSessionViewMode('focus');
  terminalLifecycle.created.length = 0;
  terminalLifecycle.destroyed.length = 0;
  terminalLifecycle.terminals.clear();
  fixture.screens.clear();
  fixture.invoke.mockClear();
  fixture.writeToPty.mockClear();
  fixture.setAgentEvents({
    sessionId: session.id,
    feeds: { queen: { agent_id: 'queen', capabilities: { can_interrupt: true, supports_approvals: false, has_terminal: true, event_source: 'transcript' } } },
    histories: { queen: { events: [{ agent_id: 'queen', cursor: 'first', source_id: 'first', kind: 'assistant_message', ts: session.created_at, truncated: false, payload: { text: 'Ready' } }], before: null, hasMore: false, loading: false, error: null } },
    connected: true,
    error: null,
  });
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  const terminalSource = readFileSync('src/lib/components/Terminal.svelte', 'utf8');
  for (const [, token] of terminalSource.matchAll(/'(--[\w-]+)'/g)) document.documentElement.style.setProperty(token, '#aaaaaa');
});

afterEach(async () => {
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  cleanup();
  document.documentElement.removeAttribute('style');
  vi.unstubAllGlobals();
});

it('G-one-terminal: Chat → Split → Terminal → Chat keeps one terminal per agent and delivers input', async () => {
  const view = render(TerminalGrid, { session, focusedAgentId: 'queen', onSelect: vi.fn() });
  const panes = await waitFor(() => session.agents.map(({ id }) => {
    const pane = view.container.querySelector<HTMLElement>(`[data-agent-id="${id}"]`)!;
    const textarea = pane.querySelector<HTMLTextAreaElement>('.xterm-helper-textarea')!;
    expect(fixture.screens.get(textarea)?.scrollback).toBe(`replayed output for ${id}`);
    return {
      id, pane, textarea,
      counter: terminalLifecycle.terminals.get(id)!.node,
    };
  }));
  const queen = within(panes[0].pane);
  const textarea = panes[0].textarea;

  function assertOneTerminal(mode: string) {
    expect(queen.getByRole('button', { name: mode }).getAttribute('aria-pressed')).toBe('true');
    for (const { id, pane, counter, textarea } of panes) {
      expect(terminalLifecycle.created.filter((agent) => agent === id), `${mode}: created for ${id}`).toHaveLength(1);
      expect(terminalLifecycle.destroyed.filter((agent) => agent === id), `${mode}: destroyed for ${id}`).toHaveLength(0);
      expect(terminalLifecycle.terminals.get(id)?.node).toBe(counter);
      expect(counter.textContent).toBe(`scrollback:${id}`);
      expect(pane.querySelector('.xterm-helper-textarea')).toBe(textarea);
      expect(fixture.screens.get(textarea)?.scrollback).toBe(`replayed output for ${id}`);
    }
    expect(terminalLifecycle.created).toHaveLength(panes.length);
    expect(terminalLifecycle.terminals.get('queen')!.props.isVisible).toBe(true);
  }

  assertOneTerminal('Chat');
  expect(textarea.closest<HTMLElement>('.terminal-layer')!.inert).toBe(true);
  const editor = queen.getByRole('textbox', { name: /Message/ });
  editor.focus();
  editor.textContent = 'chat text';
  await fireEvent.input(editor);
  await fireEvent.keyDown(editor, { key: 'Enter' });
  await waitFor(() => expect(fixture.writeToPty.mock.calls).toEqual([['queen', 'chat text']]));
  expect(fixture.invoke).toHaveBeenCalledWith('operator_inject', {
    request: { session_id: session.id, target_agent_id: 'queen', message: 'chat text' },
  });

  for (const [mode, key] of [['Split', 's'], ['Terminal', 't']]) {
    await fireEvent.click(queen.getByRole('button', { name: mode }));
    assertOneTerminal(mode);
    expect(textarea.closest<HTMLElement>('.terminal-layer')!.inert).toBe(false);
    textarea.focus();
    await tick();
    expect(document.activeElement).toBe(textarea);
    await fireEvent.keyDown(textarea, { key });
    await waitFor(() => expect(fixture.invoke).toHaveBeenCalledWith('write_to_pty', { id: 'queen', data: key }));
  }
  expect(fixture.writeToPty.mock.calls).toEqual([['queen', 'chat text'], ['queen', 's'], ['queen', 't']]);

  await fireEvent.click(queen.getByRole('button', { name: 'Chat' }));
  assertOneTerminal('Chat');
  expect(textarea.closest<HTMLElement>('.terminal-layer')!.inert).toBe(true);
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  view.unmount();
  expect([...terminalLifecycle.destroyed].sort()).toEqual(['queen', 'worker']);
});
