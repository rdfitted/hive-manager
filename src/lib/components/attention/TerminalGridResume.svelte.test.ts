import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { Session } from '$lib/stores/sessions';
import type { AgentEventsState } from '$lib/stores/agentEvents';

const fixture = vi.hoisted(() => ({
  terminals: [] as { buffer: string; disposed: boolean; textarea: HTMLTextAreaElement; keyHandler: (event: KeyboardEvent) => boolean }[],
  webglCreated: 0, webglDisposed: 0, terminalsDisposed: 0, focuses: 0,
  setAgentEvents: null as ((value: AgentEventsState) => void) | null,
  blockedReasons: {} as Record<string, string>,
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
    activeAgents: writable([]), activeSession: writable(null), sessions: { ...writable({ sessions: [] }), resumeAgent: (sessionId: string, agentId: string) => fixture.invoke('resume_agent', { sessionId, agentId } as never) },
    resumeBlockedReason: (_session: Session, id: string) => fixture.blockedReasons[id] ?? null,
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
  fixture.blockedReasons = {};
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

describe('G49 pane resume', () => {
  it.each(['grid', 'focus'] as const)('G61 live blocked notices are hidden for Running and ordinary waiting agents in %s', async (mode) => {
    layout.setSessionViewMode(mode);
    fixture.blockedReasons.claude = 'Live seat must not display a Resume refusal.';
    const live: Session = { ...session, agents: [{ ...session.agents[0], provider_session_id: 'provider-live' }] };
    const view = render(TerminalGrid, { session: live, focusedAgentId: 'claude', onSelect: vi.fn() });
    for (const status of ['Running', { WaitingForInput: 'trust_prompt' }] as const) {
      await view.rerender({ session: { ...live, agents: [{ ...live.agents[0], status }] } });
      expect(view.queryByRole('button', { name: 'Resume Claude agent' })).toBeNull();
      expect(view.queryByText('Live seat must not display a Resume refusal.')).toBeNull();
    }
  });
  it.each(['grid', 'focus'] as const)('G61 blocked Resume in %s shows its reason, while an unblocked Completed seat stays resumable', async (mode) => {
    layout.setSessionViewMode(mode);
    fixture.blockedReasons.claude = 'Saved working directory is unavailable.';
    const saved: Session = { ...session, agents: session.agents.map(agent => ({ ...agent, status: 'Completed' as const, provider_session_id: `provider-${agent.id}` })) };
    const view = render(TerminalGrid, { session: saved, focusedAgentId: 'claude', onSelect: vi.fn() });
    expect(view.queryByRole('button', { name: 'Resume Claude agent' })).toBeNull();
    expect(view.getByText('Saved working directory is unavailable.')).toBeTruthy();
    await view.rerender({ focusedAgentId: 'codex' });
    expect(view.getByRole('button', { name: 'Resume Codex PTY' })).toBeTruthy();
    expect(view.container.querySelector('button[aria-label="Resume Claude agent"]')).toBeNull();
  });
  it('shows an ellipsis while Resume is pending and restores the idle label', async () => {
    layout.setSessionViewMode('focus');
    let finishResume!: () => void;
    const resumable: Session = { ...session, agents: [
      { ...session.agents[0], status: 'Completed', provider_session_id: '00000000-0000-4000-8000-000000000001' },
    ] };
    const view = render(TerminalGrid, { session: resumable, focusedAgentId: 'claude', onSelect: vi.fn() });
    const resume = view.getByRole('button', { name: 'Resume Claude agent' }) as HTMLButtonElement;
    fixture.invoke.mockImplementationOnce(() => new Promise<undefined>((resolve) => {
      finishResume = () => resolve(undefined);
    }));
    await fireEvent.click(resume);
    expect(resume.disabled).toBe(true);
    expect(resume.textContent?.trim()).toBe('Resuming\u2026');
    finishResume();
    await waitFor(() => {
      expect(resume.disabled).toBe(false);
      expect(resume.textContent?.trim()).toBe('Resume');
    });
  });

  it.each(['focus', 'grid'] as const)('shows Resume only for an eligible finished agent in %s', async (mode) => {
    layout.setSessionViewMode(mode);
    const resumable: Session = { ...session, agents: [
      { ...session.agents[0], status: 'Completed', provider_session_id: '00000000-0000-4000-8000-000000000001' },
      { ...session.agents[1], status: 'Completed' },
    ] };
    const select = vi.fn();
    const view = render(TerminalGrid, { session: resumable, focusedAgentId: 'claude', onSelect: select });
    const resume = view.getByRole('button', { name: 'Resume Claude agent' });
    expect(view.queryByRole('button', { name: 'Resume Codex PTY', hidden: true })).toBeNull();
    const mousedown = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    resume.dispatchEvent(mousedown);
    expect(mousedown.defaultPrevented, 'resume preserves composer focus').toBe(true);
    await fireEvent.click(resume);
    expect(fixture.invoke).toHaveBeenCalledWith('resume_agent', { sessionId: 's', agentId: 'claude' });
    expect(select, 'resume does not select another pane').not.toHaveBeenCalled();
  });

  it('hides Resume for a currently running agent even when its provider ID is known', () => {
    const running = { ...session, agents: session.agents.map((agent) => ({ ...agent, provider_session_id: 'known' })) };
    const view = render(TerminalGrid, { session: running, focusedAgentId: 'claude', onSelect: vi.fn() });
    expect(view.queryByRole('button', { name: /^Resume /, hidden: true })).toBeNull();
  });

  it.each(['focus', 'grid'] as const)('offers Resume for an operator-stopped agent in %s, while ordinary waiting remains live', (mode) => {
    layout.setSessionViewMode(mode);
    const stopped: Session = { ...session, agents: session.agents.map((agent, index) => ({
      ...agent, provider_session_id: '00000000-0000-4000-8000-000000000001',
      status: { WaitingForInput: index === 0 ? 'stopped_by_operator' : 'waiting_for_answer' },
    })) };
    const view = render(TerminalGrid, { session: stopped, focusedAgentId: 'claude', onSelect: vi.fn() });
    expect(view.getByRole('button', { name: 'Resume Claude agent' })).toBeTruthy();
    expect(view.queryByRole('button', { name: 'Resume Codex PTY', hidden: true })).toBeNull();
  });
});
