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
  document.head.querySelectorAll('style[data-chat-test]').forEach((node) => node.remove()); vi.unstubAllGlobals();
});

const session: Session = {
  id: 's', session_type: { Hive: { worker_count: 1 } }, state: 'Running', project_path: 'D:/fixture', worktree_path: 'D:/fixture/cell', created_at: '2026-01-01T00:00:00Z',
  agents: ['claude', 'codex'].map((id) => ({ id, role: 'Queen', parent_id: null, status: 'Running', config: { cli: id, flags: [], label: id === 'claude' ? 'Claude agent' : 'Codex PTY' } })),
};

describe('real Terminal component behind chat (renderer doubles)', () => {
  it.each(['focus', 'grid'] as const)('T15: %s Codex transcript keeps its terminal until the first record without remount or focus', async (mode) => {
    layout.setSessionViewMode(mode);
    const publishCodex = (withRecord: boolean) => fixture.setAgentEvents!({
      sessionId: 's', feeds: { codex: { agent_id: 'codex', capabilities: { can_interrupt: true, supports_approvals: false, has_terminal: true, event_source: 'transcript' } } },
      histories: { codex: { events: withRecord ? [{ agent_id: 'codex', cursor: 'codex-first', source_id: 'codex-first', kind: 'assistant_message', ts: '2026-01-01T00:00:00Z', truncated: false, payload: { text: 'Fixture Codex transcript' } }] : [], before: null, hasMore: false, loading: false, error: null } },
      connected: true, error: null,
    });
    publishCodex(false);
    const view = render(TerminalGrid, { session, focusedAgentId: 'codex', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    const toggle = view.getByRole('button', { name: 'Terminal' });
    expect(toggle.getAttribute('aria-pressed')).toBe('true');
    const terminal = fixture.terminals[1];
    expect(terminal.textarea.closest<HTMLElement>('.terminal-layer')!.inert).toBe(false);
    expect(fixture.focuses).toBe(0);
    publishCodex(true); await tick();
    expect(terminal.textarea.closest<HTMLElement>('.terminal-layer')!.inert).toBe(true);
    expect(view.getByRole('button', { name: 'Chat' }).getAttribute('aria-pressed')).toBe('true');
    expect(fixture.terminals).toEqual(original);
    expect(original.every((term) => !term.disposed)).toBe(true);
    expect(fixture.focuses).toBe(0);
  });

  it.each(['focus', 'grid'] as const)('F3: %s empty PTY feed reveals startup prompts until the first record without focus or remount', async (mode) => {
    layout.setSessionViewMode(mode); publishFeed(false);
    const view = render(TerminalGrid, { session, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const original = [...fixture.terminals];
    const layer = view.container.querySelector<HTMLElement>('.terminal-layer')!;
    expect(layer.inert, 'empty transcript PTY must default to terminal').toBe(false);
    expect(view.getByRole('button', { name: 'Terminal' }).getAttribute('aria-pressed')).toBe('true');
    expect(view.queryByRole('region', { name: 'Agent needs attention' })).toBeNull();
    expect(fixture.focuses).toBe(0);
    publishFeed(); await tick();
    expect(layer.inert).toBe(true);
    expect(view.getByRole('button', { name: 'Chat' }).getAttribute('aria-pressed')).toBe('true');
    expect(fixture.terminals).toEqual(original);
    expect(fixture.focuses).toBe(0);
  });

  it.each(['chat', 'terminal'] as const)('F3: preserves an explicit %s choice when the first transcript arrives', async (choice) => {
    publishFeed(false);
    const view = render(TerminalGrid, { session, focusedAgentId: 'claude', onSelect: vi.fn() });
    await fireEvent.click(view.getByRole('button', { name: choice === 'chat' ? 'Chat' : 'Terminal' }));
    publishFeed(); await tick();
    expect(view.getByRole('button', { name: choice === 'chat' ? 'Chat' : 'Terminal' }).getAttribute('aria-pressed')).toBe('true');
  });

  it('F3: an archived empty transcript remains terminal-only without a feed error', async () => {
    publishFeed(false);
    const view = render(TerminalGrid, { session: { ...session, state: 'Completed' }, focusedAgentId: 'claude', onSelect: vi.fn() });
    await tick();
    expect(view.container.querySelector<HTMLElement>('.terminal-layer')!.inert).toBe(false);
    expect(view.queryByRole('region', { name: 'Agent needs attention' })).toBeNull();
  });

  it.each(['focus', 'grid'] as const)('G9: %s twenty toggles keep xterm scrollback, instances and WebGL allocation count flat', async (mode) => {
    layout.setSessionViewMode(mode);
    const view = render(TerminalGrid, { session, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals.filter((term) => term.buffer === 'fixture scrollback')).toHaveLength(2));
    const original = [...fixture.terminals];
    const allocations = fixture.webglCreated;
    expect(allocations).toBeGreaterThan(0);
    const editor = view.getByRole('textbox', { name: 'Message this agent…' }); editor.focus();
    const focuses = fixture.focuses;
    for (let index = 0; index < 20; index++) {
      await fireEvent.click(view.getByRole('button', { name: index % 2 === 0 ? 'Terminal' : 'Chat' }));
      await tick();
      expect(fixture.terminals).toEqual(original);
      expect(original.every((term) => !term.disposed && term.buffer === 'fixture scrollback')).toBe(true);
      expect(fixture.webglCreated).toBe(allocations);
      expect(fixture.webglDisposed).toBe(0);
      expect(fixture.focuses).toBe(focuses);
    }
    // Positive teardown control makes the allocation/disposal assertions meaningful.
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    cleanup();
    expect(original.every((term) => term.disposed)).toBe(true);
    expect(fixture.webglDisposed).toBe(allocations);
  });

  it('flags a dead seat in a grid pane that has no chat feed, and keeps its terminal mounted', async () => {
    layout.setSessionViewMode('grid'); publishFeed(false);
    const dead = { ...session, agents: session.agents.map((agent) => agent.id === 'codex' ? { ...agent, status: { Error: 'exited_during_startup:101' } } : agent) };
    const view = render(TerminalGrid, { session: dead, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const notice = view.getByRole('region', { name: 'Agent needs attention' });
    expect(notice.textContent).toContain('exited during startup with code 101');
    expect(fixture.terminals.every((term) => !term.disposed)).toBe(true);
  });

  it('preserves role, status and controls under the production forced-colours CSS fixture', async () => {
    layout.setSessionViewMode('grid');
    const sheet = document.createElement('style'); sheet.dataset.chatTest = 'forced-colours';
    sheet.textContent = readFileSync('src/lib/styles/globals/surfaces.css', 'utf8');
    document.head.append(sheet);
    const rules = Array.from(sheet.sheet!.cssRules).flatMap((rule) => rule instanceof CSSMediaRule
      ? Array.from(rule.cssRules).map((child) => child.cssText) : []);
    sheet.textContent = rules.join('\n');
    const waiting = { ...session, agents: session.agents.map((agent) => agent.id === 'claude' ? { ...agent, status: { WaitingForInput: 'fixture trust prompt' } } : agent) };
    const view = render(TerminalGrid, { session: waiting, focusedAgentId: 'claude', onSelect: vi.fn() });
    await tick();
    expect(view.getByText('Claude agent')).toBeTruthy();
    expect(view.getByText('WaitingForInput')).toBeTruthy();
    const notice = view.getByRole('region', { name: 'Agent needs attention' });
    expect(notice.textContent).toContain('Agent needs input.');
    for (const element of [notice, view.getByRole('region', { name: 'Agent chat' }), within(notice).getByRole('button', { name: 'Open terminal' })]) {
      expect(getComputedStyle(element).borderTopWidth).toBe('1px');
      expect(getComputedStyle(element).borderTopStyle).toBe('solid');
    }
    expect(view.getByRole('button', { name: 'Terminal' })).toBeTruthy();
    // This pins sizing/hit-test policy; jsdom does not measure desktop geometry.
    const source = readFileSync('src/lib/components/TerminalGrid.svelte', 'utf8');
    expect(source).toContain('grid-template: minmax(0, 1fr) / minmax(0, 1fr)');
    expect(source).toContain('transform: translateX(-200vw)');
    expect(source).not.toMatch(/\.terminal-layer\.chat-hidden\s*\{[^}]*display:\s*none/);
  });

  it('enables document-targeted paste only after an explicit focus inside the revealed terminal', async () => {
    layout.setSessionViewMode('focus');
    const view = render(TerminalGrid, { session, focusedAgentId: 'claude', onSelect: vi.fn() });
    await waitFor(() => expect(fixture.terminals).toHaveLength(2));
    const editor = view.getByRole('textbox', { name: 'Message this agent…' }); editor.focus();
    await fireEvent.click(view.getByRole('button', { name: 'Terminal' }));
    expect(document.activeElement).toBe(editor);
    const terminal = fixture.terminals[0]; terminal.textarea.focus(); await tick();
    // Real xterm owns paste while its helper textarea is active. Exercise the
    // document fallback with Terminal's own search input focused inside the layer.
    terminal.keyHandler(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true })); await tick();
    expect(document.activeElement).toBe(view.getByRole('textbox', { name: 'Find in terminal' }));
    const paste = new Event('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(paste, 'clipboardData', { value: { getData: () => 'dictated fixture' } });
    document.dispatchEvent(paste);
    await waitFor(() => expect(fixture.invoke).toHaveBeenCalledWith('paste_to_pty', { id: 'claude', data: 'dictated fixture' }));
    await fireEvent.click(view.getByRole('button', { name: 'Chat' }));
    fixture.invoke.mockClear();
    const hiddenPaste = new Event('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(hiddenPaste, 'clipboardData', { value: { getData: () => 'hidden fixture' } });
    document.dispatchEvent(hiddenPaste); await tick();
    expect(fixture.invoke).not.toHaveBeenCalledWith('paste_to_pty', expect.anything());
  });
});
