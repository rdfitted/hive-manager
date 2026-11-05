import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { Session } from '$lib/stores/sessions';
import type { AgentEventsState } from '$lib/stores/agentEvents';

const fixture = vi.hoisted(() => ({
  terminals: [] as { buffer: string; disposed: boolean; textarea: HTMLTextAreaElement; keyHandler: (event: KeyboardEvent) => boolean }[],
  webglCreated: 0, webglDisposed: 0, terminalsDisposed: 0, focuses: 0,
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

describe('attention across pane modes (real Terminal, renderer doubles)', () => {
  it('G59 scroll: maximize and Focus clear horizontal offset, restore retains the wall position and terminal instances', async () => {
    layout.setSessionViewMode('grid');
    const view = render(TerminalGrid, { session, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    const wall = view.container.querySelector<HTMLElement>('.terminal-grid')!;
    wall.scrollLeft = 500;
    await fireEvent.click(view.getByRole('button', { name: 'Maximize Claude agent' }));
    expect(wall.scrollLeft).toBe(0);
    await fireEvent.click(view.getByRole('button', { name: 'Restore Claude agent' }));
    await tick(); expect(wall.scrollLeft).toBe(500);
    layout.setSessionViewMode('focus'); await tick();
    expect(wall.scrollLeft).toBe(0);
    expect(fixture.terminals).toEqual(original);
    expect(fixture.terminalsDisposed).toBe(0);
  });
  it('CR-5 replacement: stable editors, per-agent drafts across both view cycles, exact routing and unchanged terminals', async () => {
    layout.setSessionViewMode('grid');
    const view = render(TerminalGrid, { session, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    const pane = view.container.querySelector<HTMLElement>('[data-agent-id="claude"]')!;
    const shared = () => view.container.querySelector<HTMLElement>('.command-bar [data-composer]')!;
    const focus = () => pane.querySelector<HTMLElement>('.chat-composer [data-composer]')!;
    const type = async (editor: HTMLElement, text: string) => { editor.textContent = text; await fireEvent.input(editor); };
    const gridEditor = shared();
    expect(view.container.querySelectorAll('.terminal-item .chat-composer')).toHaveLength(0);
    expect(view.container.querySelectorAll('.command-bar [data-composer]')).toHaveLength(1);
    await type(gridEditor, 'Alpha private draft');
    const waiting = { ...session, agents: session.agents.map(agent => agent.id === 'claude' ? { ...agent, status: { WaitingForInput: 'injected_delivery_failed:fixture' } } : agent) };
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
    expect(focusEditor.textContent).toBe('Alpha private draft');
    await type(focusEditor, 'Alpha Focus draft');
    await view.rerender({ session });
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
    await fireEvent.click(view.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(fixture.invoke).toHaveBeenCalledWith('operator_inject', { request: { session_id: 's', target_agent_id: 'codex', message: 'Beta private draft' } }));
    expect(fixture.invoke.mock.calls.filter(([command]) => command === 'operator_inject')).toHaveLength(1);
    await fireEvent.click(view.getByRole('button', { name: 'Focus Claude agent' }));
    expect(shared().textContent).toBe('Alpha Focus draft');
    layout.setSessionViewMode('focus'); await tick();
    expect(focus().textContent).toBe('Alpha Focus draft');
    expect(fixture.terminals).toHaveLength(original.length);
    original.forEach((terminal, index) => { expect(fixture.terminals[index]).toBe(terminal); expect(terminal.disposed).toBe(false); });
    expect(fixture.terminalsDisposed).toBe(0);
    expect(fixture.focuses).toBe(0);
  });

  it('G59: Queen leads the full roster, column clicks set the shared target and resource tiles highlight without unmounting', async () => {
    layout.setSessionViewMode('grid');
    const reordered = { ...session, state: 'QaInProgress' as const, agents: [...session.agents].reverse().map(agent => ({ ...agent, role: agent.id === 'claude' ? 'Queen' as const : { Worker: { index: 1, parent: null } }, status: agent.id === 'codex' ? { WaitingForInput: 'fixture' } : 'Running' as const })) };
    const view = render(TerminalGrid, { session: reordered, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    expect([...view.container.querySelectorAll('.terminal-item')].map(node => node.getAttribute('data-agent-id'))).toEqual(['claude', 'codex']);
    expect(view.container.querySelector('.terminal-item')?.classList.contains('queen-column')).toBe(true);
    await fireEvent.click(view.getByRole('button', { name: 'Focus Codex PTY' }));
    expect(view.getByRole('button', { name: 'Command target: @Codex PTY' })).toBeTruthy();
    expect(view.getByText('QaInProgress')).toBeTruthy();
    await fireEvent.click(view.getByRole('button', { name: 'Waiting 1' }));
    expect(view.container.querySelectorAll('.resource-match')).toHaveLength(1);
    expect(view.container.querySelector('.resource-match')?.getAttribute('data-agent-id')).toBe('codex');
    await fireEvent.click(view.getByRole('button', { name: 'Waiting 1' }));
    expect(view.container.querySelectorAll('.resource-match')).toHaveLength(0);
    expect(fixture.terminals).toEqual(original);
    expect(fixture.terminalsDisposed).toBe(0);
  });

  it('G59 quiet seats dim in place with no terminal disposal or finished attention', async () => {
    layout.setSessionViewMode('grid');
    const view = render(TerminalGrid, { session, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    await view.rerender({ session: { ...session, agents: session.agents.map(agent => ({ ...agent, status: agent.id === 'claude' ? 'Completed' as const : 'Running' as const, work_state: agent.id === 'codex' ? 'idle' : undefined })) } });
    expect(view.container.querySelectorAll('.quiet-column')).toHaveLength(2);
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
      expect(term.buffer).toContain('fixture scrollback');
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
    let composer = view.container.querySelector('.command-bar [role="textbox"]');
    expect(composer).toBeTruthy();
    expect(content.classList.contains('non-chat-attention'), 'chat panes retain their composer layout').toBe(false);
    expect(content.querySelector('.agent-info'), 'no opaque wrapper or padding without attention').toBeNull();
    expect(pane.querySelector('.terminal-header .activity-line'), 'activity belongs in the existing header').toBeTruthy();
    expect(content.querySelector('.activity-line')).toBeNull();
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
      expect(next === 'focus' ? pane.querySelector('.chat-composer [role="textbox"]') : view.container.querySelector('.command-bar [role="textbox"]')).toBeTruthy();
      if (next === 'grid') composer = view.container.querySelector('.command-bar [role="textbox"]');
      expect(fixture.terminals).toHaveLength(original.length);
      original.forEach((term, index) => expect(fixture.terminals[index]).toBe(term));
      expect(original.every((term) => !term.disposed)).toBe(true);
    }
    await view.rerender({ session });
    expect(content.querySelector('.agent-info')).toBeNull();
    expect(within(pane as HTMLElement).queryByRole('region', { name: 'Agent needs attention' })).toBeNull();
    expect(view.container.querySelector('.command-bar [role="textbox"]')).toBe(composer);
    expect(pane.querySelector('.chat-composer')).toBeNull();
    original.forEach((term, index) => expect(fixture.terminals[index]).toBe(term));
    expect(original.every((term) => !term.disposed)).toBe(true);
    expect(fixture.focuses).toBe(0);
  });

});
