import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import type { Session } from '$lib/stores/sessions';
import type { AgentEventsState } from '$lib/stores/agentEvents';

const fixture = vi.hoisted(() => ({
  terminals: [] as { scrollback: string; disposed: boolean; textarea: HTMLTextAreaElement; keyHandler: (event: KeyboardEvent) => boolean }[],
  webglCreated: 0, webglDisposed: 0, terminalsDisposed: 0, focuses: 0,
  setAgentEvents: null as ((value: AgentEventsState) => void) | null,
  invoke: vi.fn(async (command: string, args: { id: string }) => command === 'get_pty_snapshot'
    ? { id: args.id, offset_start: 0, offset_end: 17, data: btoa('fixture scrollback') } : undefined),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('phosphor-svelte', () => ({
  ArrowUpRight: () => {}, Bell: () => {}, ChatCircle: () => {}, ListBullets: () => {}, SquareSplitVertical: () => {}, TerminalWindow: () => {}, Timer: () => {},
  ArrowDown: () => {}, ArrowUp: () => {}, Broom: () => {}, CaretDown: () => {}, CaretUp: () => {}, CheckSquare: () => {},
  ClipboardText: () => {}, FileText: () => {}, MagnifyingGlass: () => {}, X: () => {},
  ArrowsIn: () => {}, ArrowsOut: () => {}, Check: () => {}, Circle: () => {}, Hourglass: () => {}, Plus: () => {},
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ onDragDropEvent: vi.fn().mockResolvedValue(() => {}) }) }));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ writeText: vi.fn(), readText: vi.fn() }));
vi.mock('@xterm/xterm', () => ({ Terminal: class {
  scrollback = ''; disposed = false; textarea = document.createElement('textarea');
  keyHandler = (_event: KeyboardEvent) => true;
  constructor(public options: Record<string, unknown>) { fixture.terminals.push(this); }
  open(node: HTMLElement) { this.textarea.className = 'xterm-helper-textarea'; node.append(this.textarea); }
  get buffer() {
    const lines = this.scrollback.split('\n');
    return { active: { baseY: 0, cursorY: lines.length - 1, getLine: (row: number) => ({ translateToString: () => lines[row].trimEnd() }) } };
  }
  write(text: string, afterWrite?: () => void) { this.scrollback += text; afterWrite?.(); }
  focus() { fixture.focuses++; this.textarea.focus(); }
  loadAddon() {}
  onData() { return { dispose() {} }; }
  attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean) { this.keyHandler = handler; }
  getSelection() { return ''; }
  scrollToBottom() {}
  dispose() { this.disposed = true; fixture.terminalsDisposed++; }
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
    resumeBlockedReason: () => null,
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
  fixture.terminals.length = 0; fixture.webglCreated = 0; fixture.webglDisposed = 0; fixture.terminalsDisposed = 0; fixture.focuses = 0;
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

const stageRoster: Session = {
  ...session,
  agents: [session.agents[0], { ...session.agents[1], role: { Worker: { index: 1, parent: null } } }],
};

function publishModeFeed(hasTerminal: boolean) {
  const feeds: AgentEventsState['feeds'] = {};
  const histories: AgentEventsState['histories'] = {};
  for (const { id } of stageRoster.agents) {
    feeds[id] = {
      agent_id: id,
      capabilities: { has_terminal: hasTerminal, can_interrupt: true, supports_approvals: !hasTerminal, event_source: hasTerminal ? 'transcript' : 'protocol' },
    };
    histories[id] = {
      events: [{ agent_id: id, cursor: 'mode-test', source_id: 'mode-test', kind: 'assistant_message',
        ts: '2026-01-01T00:00:00Z', truncated: false, payload: { text: `Transcript for ${id}` } }],
      before: null, hasMore: false, loading: false, error: null,
    };
  }
  fixture.setAgentEvents!({ sessionId: session.id, feeds, histories, connected: true, error: null });
}

describe('attention across pane modes (real Terminal, renderer doubles)', () => {
  it('shows the Queen Focus chat after worker selection through the input and the pane picker', async () => {
    vi.stubGlobal('innerWidth', 800);
    publishModeFeed(false);
    layout.setSessionViewMode('grid');
    layout.setMaximizedTerminalId('codex');
    const onSelect = vi.fn();
    const view = render(TerminalGrid, { session: stageRoster, focusedAgentId: 'codex', onSelect });
    layout.setSessionViewMode('focus');
    await tick();
    const picker = view.getByRole('combobox', { name: 'Focused terminal' }) as HTMLSelectElement;
    const queen = view.container.querySelector<HTMLElement>('.queen-column')!;
    const chat = queen.querySelector<HTMLElement>('.chat-pane')!;
    const assertQueenVisible = () => {
      expect(picker.value).toBe('claude');
      expect(queen.closest('.terminal-grid')?.classList.contains('queen-only')).toBe(true);
      expect(queen.classList.contains('hidden-by-maximize')).toBe(false);
      expect(queen.closest('[hidden], [inert]')).toBeNull();
      expect(chat.closest('[hidden], [inert], [aria-hidden="true"]')).toBeNull();
      expect(within(queen).getByRole('region', { name: 'Agent chat' })).toBe(chat);
    };

    expect(picker.value).toBe('codex');
    expect(queen.classList.contains('hidden-by-maximize')).toBe(true);
    await view.rerender({ focusedAgentId: 'claude' });
    assertQueenVisible();

    await view.rerender({ focusedAgentId: 'codex' });
    expect(queen.classList.contains('hidden-by-maximize')).toBe(true);
    await fireEvent.change(picker, { target: { value: 'claude' } });
    expect(onSelect).toHaveBeenLastCalledWith('claude');
    await view.rerender({ focusedAgentId: 'claude' });
    assertQueenVisible();
  });

  it('shows Chat, Split and Terminal for the Queen and worker and keeps the parsed line when the worker collapses', async () => {
    layout.setSessionViewMode('grid');
    publishModeFeed(true);
    const view = render(TerminalGrid, { session: stageRoster, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals.map(term => term.scrollback)).toEqual(['fixture scrollback', 'fixture scrollback']));

    for (const { id } of stageRoster.agents) {
      const pane = view.container.querySelector<HTMLElement>(`[data-agent-id="${id}"]`)!;
      const body = pane.querySelector<HTMLElement>('.agent-pane-body')!;
      const chat = body.querySelector<HTMLElement>('.chat-layer')!;
      const terminal = body.querySelector<HTMLElement>('.terminal-layer')!;
      const buttons = within(pane).getByRole('group', { name: 'Agent pane' });
      const select = async (label: string) => {
        const button = within(buttons).getByRole('button', { name: label });
        button.focus();
        await fireEvent.click(button);
        expect(document.activeElement).toBe(button);
        expect(button.getAttribute('aria-pressed')).toBe('true');
      };

      expect(body.dataset.mode).toBe('chat');
      expect([chat.inert, terminal.inert]).toEqual([false, true]);
      await select('Split');
      expect(body.dataset.mode).toBe('split');
      expect([chat.inert, terminal.inert]).toEqual([false, false]);
      await select('Terminal');
      expect(body.dataset.mode).toBe('terminal');
      expect([chat.inert, terminal.inert]).toEqual([true, false]);
      await select('Chat');
      expect(body.dataset.mode).toBe('chat');
      expect([chat.inert, terminal.inert]).toEqual([false, true]);
    }

    const original = [...fixture.terminals];
    const worker = view.container.querySelector<HTMLElement>('[data-agent-id="codex"]')!;
    await fireEvent.click(within(worker).getByRole('button', { name: 'Collapse Codex PTY' }));
    expect(worker.querySelector('.terminal-status-line.compact')?.textContent).toContain('fixture scrollback');
    expect(worker.querySelector('.terminal-container')?.hasAttribute('hidden')).toBe(true);
    expect(worker.querySelectorAll('.xterm-helper-textarea')).toHaveLength(1);
    expect(fixture.terminals).toEqual(original);
    expect(fixture.terminalsDisposed).toBe(0);
  });

  it.each(['grid', 'focus'] as const)('opens Split from the parsed terminal line above the %s Queen composer', async (mode) => {
    layout.setSessionViewMode(mode);
    publishModeFeed(true);
    const view = render(TerminalGrid, { session: stageRoster, focusedAgentId: 'claude', onSelect: vi.fn() });
    const pane = view.container.querySelector<HTMLElement>('[data-agent-id="claude"]')!;
    const status = pane.querySelector<HTMLElement>('.chat-composer .terminal-status-line')!;
    const editor = within(pane).getByRole('textbox');
    expect(editor.getAttribute('aria-disabled')).toBe('false');
    expect(status.compareDocumentPosition(editor) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    if (mode === 'grid') {
      expect(view.container.querySelectorAll('[data-composer]')).toHaveLength(1);
      expect(within(pane).getByRole('button', { name: 'Command target: @Claude agent' }).textContent?.trim()).toBe('@Claude agent');
    }
    await waitFor(() => expect(status.textContent).toContain('fixture scrollback'));
    const button = within(status).getByRole('button', { name: 'Open Split' });
    button.focus();
    await fireEvent.click(button);
    expect(pane.querySelector<HTMLElement>('.agent-pane-body')!.dataset.mode).toBe('split');
    expect(document.activeElement).toBe(within(pane).getByRole('button', { name: 'Split' }));
    expect(fixture.focuses).toBe(0);
  });

  it('opens Raw for the Queen and worker when their protocol feeds have no terminal', async () => {
    layout.setSessionViewMode('grid');
    publishModeFeed(false);
    const view = render(TerminalGrid, { session: stageRoster, focusedAgentId: 'claude', onSelect: vi.fn() });

    for (const { id } of stageRoster.agents) {
      const pane = view.container.querySelector<HTMLElement>(`[data-agent-id="${id}"]`)!;
      const ui = within(pane);
      expect(ui.queryByRole('button', { name: 'Terminal' })).toBeNull();
      expect(ui.queryByRole('button', { name: 'Split' })).toBeNull();
      await fireEvent.click(ui.getByRole('button', { name: 'Raw event log' }));
      expect(ui.getByRole('region', { name: 'Raw agent event log' }).textContent).toContain(`Transcript for ${id}`);
      expect(pane.querySelector<HTMLElement>('.agent-pane-body')!.dataset.mode).toBe('raw');
    }
    expect(fixture.terminals).toHaveLength(0);
  });

  it('G59 scroll: restore returns to the worker stack position with the same terminal instances', async () => {
    layout.setSessionViewMode('grid');
    const roster = { ...session, agents: [session.agents[0], { ...session.agents[1], role: { Worker: { index: 1, parent: null } } }] };
    const view = render(TerminalGrid, { session: roster, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    const workers = view.container.querySelector<HTMLElement>('.worker-viewport')!;
    workers.scrollTop = 125;

    await fireEvent.click(view.getByRole('button', { name: 'Maximize Claude agent' }));
    expect(workers.hidden).toBe(true);
    workers.scrollTop = 0;
    await fireEvent.click(view.getByRole('button', { name: 'Restore Claude agent' }));
    await tick();
    expect(workers.hidden).toBe(false);
    expect(workers.scrollTop).toBe(125);

    layout.setSessionViewMode('focus');
    await tick();
    expect(workers.hidden).toBe(true);
    workers.scrollTop = 0;
    layout.setSessionViewMode('grid');
    await tick();
    expect(workers.scrollTop).toBe(125);
    expect(fixture.terminals).toEqual(original);
    expect(fixture.terminalsDisposed).toBe(0);
  });

  it('CR-5 replacement: stable editors, per-agent drafts across both view cycles, exact routing and unchanged terminals', async () => {
    layout.setSessionViewMode('grid');
    const view = render(TerminalGrid, { session: stageRoster, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    const pane = view.container.querySelector<HTMLElement>('[data-agent-id="claude"]')!;
    const shared = () => pane.querySelector<HTMLElement>('.chat-composer [data-composer]')!;
    const focus = shared;
    const type = async (editor: HTMLElement, text: string) => { editor.textContent = text; await fireEvent.input(editor); };
    const gridEditor = shared();
    expect(view.container.querySelectorAll('.terminal-item .chat-composer')).toHaveLength(1);
    expect(view.container.querySelector('.worker-viewport .chat-composer')).toBeNull();
    await type(gridEditor, 'Alpha private draft');
    const waiting = { ...stageRoster, agents: stageRoster.agents.map(agent => agent.id === 'claude' ? { ...agent, status: { WaitingForInput: 'injected_delivery_failed:fixture' } } : agent) };
    await view.rerender({ session: waiting });
    expect(shared()).toBe(gridEditor);
    for (const label of ['Terminal', 'Chat']) {
      await fireEvent.click(within(pane).getByRole('button', { name: label }));
      expect(shared()).toBe(gridEditor);
    }
    await fireEvent.click(within(pane).getByRole('button', { name: 'Maximize Claude agent' }));
    expect(shared()).toBe(gridEditor);
    await fireEvent.click(within(pane).getByRole('button', { name: 'Restore Claude agent' }));
    expect(shared()).toBe(gridEditor);
    await fireEvent.click(view.getByRole('button', { name: 'Focus Codex PTY' }));
    expect(shared()).toBe(gridEditor);
    expect(gridEditor.textContent).toBe('');
    await type(gridEditor, 'Beta private draft');
    await fireEvent.click(view.getByRole('button', { name: 'Focus Claude agent' }));
    expect(shared()).toBe(gridEditor);
    expect(gridEditor.textContent).toBe('Alpha private draft');
    layout.setSessionViewMode('focus'); await tick();
    const focusEditor = focus();
    expect(focusEditor).toBe(gridEditor);
    expect(focusEditor.textContent).toBe('Alpha private draft');
    await type(focusEditor, 'Alpha Focus draft');
    await view.rerender({ session: stageRoster });
    expect(focus()).toBe(focusEditor);
    expect(pane.querySelector('.agent-info')).toBeNull();
    await view.rerender({ session: waiting });
    expect(focus()).toBe(focusEditor);
    expect(within(pane).getByRole('region', { name: 'Agent needs attention' })).toBeTruthy();
    for (const label of ['Terminal', 'Chat']) {
      await fireEvent.click(within(pane).getByRole('button', { name: label }));
      expect(focus()).toBe(focusEditor);
    }
    layout.setSessionViewMode('grid'); await tick();
    expect(shared().textContent).toBe('Alpha Focus draft');
    expect(within(pane).getByRole('region', { name: 'Agent needs attention' })).toBeTruthy();
    await fireEvent.click(view.getByRole('button', { name: 'Focus Codex PTY' }));
    expect(shared().textContent).toBe('Beta private draft');
    await fireEvent.click(shared());
    expect(within(pane).getByRole('button', { name: 'Command target: @Codex PTY' })).toBeTruthy();
    expect(shared().textContent).toBe('Beta private draft');
    await fireEvent.click(view.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(fixture.invoke).toHaveBeenCalledWith('operator_inject', { request: { session_id: 's', target_agent_id: 'codex', message: 'Beta private draft' } }));
    expect(fixture.invoke.mock.calls.filter(([command]) => command === 'operator_inject')).toHaveLength(1);
    await fireEvent.click(pane);
    expect(within(pane).getByRole('button', { name: 'Command target: @Claude agent' })).toBeTruthy();
    expect(shared().textContent).toBe('Alpha Focus draft');
    layout.setSessionViewMode('focus'); await tick();
    expect(focus().textContent).toBe('Alpha Focus draft');
    expect(fixture.terminals).toHaveLength(original.length);
    original.forEach((terminal, index) => { expect(fixture.terminals[index]).toBe(terminal); expect(terminal.disposed).toBe(false); });
    expect(fixture.terminalsDisposed).toBe(0);
    expect(fixture.focuses).toBe(0);
  });

  it('lets the Grid target picker consume Escape before restoring the maximized Queen', async () => {
    layout.setSessionViewMode('grid');
    const view = render(TerminalGrid, { session: stageRoster, focusedAgentId: 'claude', onSelect: vi.fn() });
    await fireEvent.click(view.getByRole('button', { name: 'Maximize Claude agent' }));
    const picker = view.getByRole('button', { name: 'Command target: @Claude agent' });
    await fireEvent.click(picker);
    expect(picker.getAttribute('aria-expanded')).toBe('true');

    await fireEvent.keyDown(picker, { key: 'Escape', bubbles: true, cancelable: true });
    expect(picker.getAttribute('aria-expanded')).toBe('false');
    expect(view.getByRole('button', { name: 'Restore Claude agent' })).toBeTruthy();

    await fireEvent.keyDown(picker, { key: 'Escape', bubbles: true, cancelable: true });
    expect(view.getByRole('button', { name: 'Maximize Claude agent' })).toBeTruthy();
  });

  it('restores a maximized Grid pane before xterm consumes Escape', async () => {
    layout.setSessionViewMode('grid');
    const view = render(TerminalGrid, { session: stageRoster, focusedAgentId: 'codex', onSelect: vi.fn() });
    const worker = view.container.querySelector<HTMLElement>('[data-agent-id="codex"]')!;
    const maximize = () => fireEvent.click(within(worker).getByRole('button', { name: 'Maximize Codex PTY' }));
    await maximize();
    const terminalInput = worker.querySelector<HTMLTextAreaElement>('textarea.xterm-helper-textarea')!;
    terminalInput.focus();
    expect(document.activeElement).toBe(terminalInput);

    const terminalKey = vi.fn((event: KeyboardEvent) => {
      const before = { maximized: get(layout).maximizedTerminalId, prevented: event.defaultPrevented };
      event.preventDefault();
      event.stopPropagation();
      return before;
    });
    const escape = () => fireEvent.keyDown(terminalInput, { key: 'Escape', bubbles: true, cancelable: true });
    const modal = document.createElement('div');
    modal.setAttribute('aria-modal', 'true');

    terminalInput.addEventListener('keydown', terminalKey, true);
    try {
      await escape();
      expect(terminalKey).toHaveBeenCalledTimes(1);
      expect(terminalKey).toHaveReturnedWith({ maximized: null, prevented: true });
      expect(within(worker).getByRole('button', { name: 'Maximize Codex PTY' })).toBeTruthy();

      await escape();
      expect(terminalKey).toHaveBeenCalledTimes(2);
      expect(terminalKey).toHaveLastReturnedWith({ maximized: null, prevented: false });

      await maximize();
      document.body.append(modal);
      await escape();
      expect(terminalKey).toHaveBeenCalledTimes(3);
      expect(terminalKey).toHaveLastReturnedWith({ maximized: 'codex', prevented: false });
      expect(within(worker).getByRole('button', { name: 'Restore Codex PTY' })).toBeTruthy();
    } finally {
      terminalInput.removeEventListener('keydown', terminalKey, true);
      modal.remove();
    }
  });

  it('G59: Queen leads the full roster, column clicks set the shared target and the resource filter highlights without unmounting', async () => {
    layout.setSessionViewMode('grid');
    const reordered = { ...session, state: 'QaInProgress' as const, agents: [...session.agents].reverse().map(agent => ({ ...agent, role: agent.id === 'claude' ? 'Queen' as const : { Worker: { index: 1, parent: null } }, status: agent.id === 'codex' ? { WaitingForInput: 'fixture' } : 'Running' as const })) };
    const view = render(TerminalGrid, { session: reordered, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    expect([...view.container.querySelectorAll('.terminal-item')].map(node => node.getAttribute('data-agent-id'))).toEqual(['claude', 'codex']);
    expect(view.container.querySelector('.terminal-item')?.classList.contains('queen-column')).toBe(true);
    await fireEvent.click(view.getByRole('button', { name: 'Focus Codex PTY' }));
    expect(view.getByRole('button', { name: 'Command target: @Codex PTY' })).toBeTruthy();
    expect(view.queryByRole('navigation', { name: 'Agent resources' })).toBeNull();
    await view.rerender({ resourceFilter: 'waiting' });
    expect(view.container.querySelectorAll('.resource-match')).toHaveLength(1);
    expect(view.container.querySelector('.resource-match')?.getAttribute('data-agent-id')).toBe('codex');
    await view.rerender({ resourceFilter: null });
    expect(view.container.querySelectorAll('.resource-match')).toHaveLength(0);
    expect(fixture.terminals).toEqual(original);
    expect(fixture.terminalsDisposed).toBe(0);
  });

  it('uses the supplied resource filter, including an explicit clear', async () => {
    layout.setSessionViewMode('grid');
    const waiting: Session = {
      ...session,
      agents: session.agents.map(agent => agent.id === 'codex'
        ? { ...agent, status: { WaitingForInput: 'awaiting_injected_turn' } }
        : agent),
    };
    const view = render(TerminalGrid, { session: waiting, focusedAgentId: 'claude', onSelect: vi.fn() });
    const matchedIds = () => [...view.container.querySelectorAll('.resource-match')]
      .map(node => node.getAttribute('data-agent-id'));

    expect(matchedIds()).toEqual([]);
    await view.rerender({ resourceFilter: 'waiting' });
    expect(matchedIds()).toEqual(['codex']);
    await view.rerender({ resourceFilter: null });
    expect(matchedIds()).toEqual([]);
    expect(view.container.querySelectorAll('.terminal-item')).toHaveLength(2);
  });

  it('focuses and scrolls on each new reveal nonce, even for the same agent', async () => {
    layout.setSessionViewMode('grid');
    const onSelect = vi.fn();
    const roster = { ...session, agents: [session.agents[0], { ...session.agents[1], role: { Worker: { index: 1, parent: null } } }] };
    const view = render(TerminalGrid, { session: roster, focusedAgentId: 'claude', onSelect });
    const header = view.getByRole('button', { name: 'Focus Codex PTY' });
    const scrollIntoView = vi.fn();
    header.scrollIntoView = scrollIntoView;

    await view.rerender({ revealRequest: { agentId: 'codex', nonce: 1 } });
    expect(onSelect.mock.calls).toEqual([['codex']]);
    await waitFor(() => expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest', inline: 'nearest', behavior: 'smooth' }));

    await view.rerender({ revealRequest: { agentId: 'claude', nonce: 1 } });
    expect(onSelect).toHaveBeenCalledTimes(1);
    await view.rerender({ revealRequest: { agentId: 'codex', nonce: 2 } });
    expect(onSelect.mock.calls).toEqual([['codex'], ['codex']]);
    await waitFor(() => expect(scrollIntoView).toHaveBeenCalledTimes(2));
  });

  it('publishes attention and stale seats when they change', async () => {
    const onSeats = vi.fn();
    const changed: Session = {
      ...session,
      agents: session.agents.map(agent => agent.id === 'claude'
        ? { ...agent, status: { Error: 'fixture error' } }
        : agent),
    };
    const view = render(TerminalGrid, { session: changed, focusedAgentId: 'claude', onSelect: vi.fn(), onSeats });
    await tick();
    expect(onSeats).toHaveBeenLastCalledWith({ attention: new Set(['claude']), stale: new Set(['codex']) });

    await view.rerender({ session: { ...changed, agents: changed.agents.map(agent => ({ ...agent, status: 'Completed' })) } });
    expect(onSeats).toHaveBeenLastCalledWith({ attention: new Set(), stale: new Set() });
  });

  it('G59 quiet workers collapse in place with no terminal disposal or finished attention', async () => {
    layout.setSessionViewMode('grid');
    const roster = { ...session, agents: [session.agents[0], { ...session.agents[1], role: { Worker: { index: 1, parent: null } } }] };
    const view = render(TerminalGrid, { session: roster, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    await view.rerender({ session: { ...roster, agents: roster.agents.map(agent => ({ ...agent, status: agent.id === 'claude' ? 'Completed' as const : 'Running' as const, work_state: agent.id === 'codex' ? 'idle' : undefined })) } });
    expect(view.container.querySelectorAll('.quiet-column')).toHaveLength(2);
    expect(view.container.querySelector('.worker-viewport .collapsed')?.getAttribute('data-agent-id')).toBe('codex');
    expect(view.container.querySelector('.queen-column.collapsed')).toBeNull();
    expect(view.container.querySelectorAll('.terminal-item')).toHaveLength(2);
    expect(view.queryByRole('region', { name: 'Agent needs attention' })).toBeNull();
    expect(fixture.terminals).toEqual(original);
    expect(fixture.terminalsDisposed).toBe(0);
  });

  it.each(['receipt', 'error'] as const)('CR-9: non-chat PTY retains pending and %s in Grid without granting Flush', async (result) => {
    layout.setSessionViewMode('focus');
    let resolve!: (response: unknown) => void;
    let reject!: (reason: Error) => void;
    const fetcher = vi.fn(() => new Promise((done, fail) => { resolve = done; reject = fail; }));
    vi.stubGlobal('fetch', fetcher);
    const waiting = { ...session, agents: session.agents.map((agent) => agent.id === 'codex'
      ? { ...agent, status: { WaitingForInput: 'injected_delivery_failed:fixture' } } : agent) };
    const view = render(TerminalGrid, { session: waiting, focusedAgentId: 'codex', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    const original = [...fixture.terminals];
    const textareas = original.map((term) => term.textarea);
    const pane = view.getByText('Codex PTY', { selector: '.role-label' }).closest('.terminal-item') as HTMLElement;
    const terminalLayer = pane.querySelector('.terminal-layer');
    const other = view.getByText('Claude agent', { selector: '.role-label' }).closest('.terminal-item')!;
    await fireEvent.click(within(pane).getByRole('button', { name: 'Flush Enter' }));
    layout.setSessionViewMode('grid'); await tick();
    const pending = within(pane).getByRole('button', { name: /^Flushing/ }) as HTMLButtonElement;
    expect(pending.disabled, 'CR-9: retained pending Flush must be accessible in non-chat Grid').toBe(true);
    expect(pane.querySelector('.terminal-container')?.classList.contains('non-chat-attention')).toBe(true);
    // Explicit view switches focus the selected non-chat PTY by its existing
    // layout contract. Receipt and attention updates must not focus it again.
    let focusesAfterLayout = fixture.focuses;
    await fireEvent.click(pending);
    expect(fetcher).toHaveBeenCalledOnce();
    const message = result === 'receipt' ? 'Enter may not have landed. Check the terminal.' : 'Error: fixture Flush failed';
    if (result === 'receipt') resolve({ ok: true, text: async () => '{"submit_confirmed":false,"target":"codex"}' });
    else reject(new Error('fixture Flush failed'));
    await waitFor(() => expect(within(pane).getByText(message)).toBeTruthy());
    expect(fixture.focuses, 'settling Flush must not steal focus').toBe(focusesAfterLayout);
    if (result === 'receipt') {
      await fireEvent.click(within(pane).getByText('Details'));
      expect(within(pane).getByLabelText('Inject receipt').textContent).toContain('"target":"codex"');
    }
    // A retained result must never become permission to Flush after eligibility clears.
    for (const control of [
      { status: 'Running' as const },
      { status: 'Running' as const, work_state: 'finished' as const },
      { status: { Error: 'fixture agent failure' } },
      { status: { Error: 'exited_during_startup:7' } },
    ]) {
      const controlled = { ...session, agents: session.agents.map((agent) => agent.id === 'codex' ? { ...agent, ...control } : agent) };
      await view.rerender({ session: controlled });
      expect(fixture.focuses, 'attention changes must not steal focus').toBe(focusesAfterLayout);
      for (const mode of ['focus', 'grid'] as const) {
        layout.setSessionViewMode(mode); await tick();
        focusesAfterLayout = fixture.focuses;
        expect(pane.querySelector('.terminal-container')?.classList.contains('non-chat-attention')).toBe(mode === 'grid');
        expect(within(pane).getByText(message)).toBeTruthy();
        expect(within(pane).queryByRole('button', { name: 'Flush Enter' })).toBeNull();
        expect(other.textContent).not.toContain(message);
        expect(pane.querySelector('.terminal-layer')).toBe(terminalLayer);
        original.forEach((term, index) => {
          expect(fixture.terminals[index]).toBe(term);
          expect(term.textarea).toBe(textareas[index]);
          expect(term.disposed).toBe(false);
        });
        expect(fixture.terminals).toHaveLength(original.length);
        expect(fixture.terminalsDisposed).toBe(0);
      }
    }
    await view.rerender({ session: { ...session, id: 'other' } });
    expect(view.queryByText(message), 'receipts are scoped to the session').toBeNull();
    await view.rerender({ session });
    expect(within(pane).getByText(message)).toBeTruthy();
    if (result === 'receipt') {
      await fireEvent.click(within(pane).getByRole('button', { name: 'Dismiss Enter receipt' }));
      expect(within(pane).queryByRole('region', { name: 'Agent needs attention' })).toBeNull();
      expect(pane.querySelector('.agent-info')).toBeNull();
      expect(pane.querySelector('.terminal-container')?.classList.contains('non-chat-attention')).toBe(false);
    }
    expect(fetcher).toHaveBeenCalledOnce();
    expect(fixture.terminalsDisposed).toBe(0);
    expect(fixture.terminals).toHaveLength(original.length);
    original.forEach((term, index) => {
      expect(fixture.terminals[index]).toBe(term);
      expect(term.textarea).toBe(textareas[index]);
      expect(term.scrollback).toContain('fixture scrollback');
    });
    expect(fixture.focuses).toBe(focusesAfterLayout);
  });

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
    let composer = view.container.querySelector('.queen-column .chat-composer [role="textbox"]');
    expect(composer).toBeTruthy();
    expect(content.classList.contains('non-chat-attention'), 'chat panes retain their composer layout').toBe(false);
    expect(content.querySelector('.agent-info'), 'no opaque wrapper or padding without attention').toBeNull();
    expect(pane.querySelector('.terminal-header .activity-line'), 'activity belongs in the existing header').toBeTruthy();
    expect([...content.querySelectorAll('.activity-line')].every(line => line.closest('.terminal-status-line'))).toBe(true);
    const waiting = { ...session, agents: session.agents.map((agent) => agent.id === 'claude'
      ? { ...agent, status: { WaitingForInput: 'injected_delivery_failed:fixture' } } : agent) };
    await view.rerender({ session: waiting });
    const warning = within(pane as HTMLElement).getByRole('region', { name: 'Agent needs attention' });
    expect(warning.closest('.agent-info')).toBeTruthy();
    expect(content.classList.contains('non-chat-attention')).toBe(false);
    expect(within(warning).getByRole('button', { name: 'Open terminal' })).toBeTruthy();
    for (const next of ['focus', 'grid'] as const) {
      layout.setSessionViewMode(next); await tick();
      expect(within(pane as HTMLElement).getByRole('region', { name: 'Agent needs attention' })).toBeTruthy();
      expect(next === 'focus' ? pane.querySelector('.chat-composer [role="textbox"]') : view.container.querySelector('.queen-column .chat-composer [role="textbox"]')).toBeTruthy();
      if (next === 'grid') composer = view.container.querySelector('.queen-column .chat-composer [role="textbox"]');
      expect(fixture.terminals).toHaveLength(original.length);
      original.forEach((term, index) => expect(fixture.terminals[index]).toBe(term));
      expect(original.every((term) => !term.disposed)).toBe(true);
    }
    await view.rerender({ session });
    expect(content.querySelector('.agent-info')).toBeNull();
    expect(within(pane as HTMLElement).queryByRole('region', { name: 'Agent needs attention' })).toBeNull();
    expect(view.container.querySelector('.queen-column .chat-composer [role="textbox"]')).toBe(composer);
    expect(pane.querySelector('.chat-composer')).toBeTruthy();
    expect(view.container.querySelectorAll('.chat-composer')).toHaveLength(1);
    original.forEach((term, index) => expect(fixture.terminals[index]).toBe(term));
    expect(original.every((term) => !term.disposed)).toBe(true);
    expect(fixture.focuses).toBe(0);
  });

});
