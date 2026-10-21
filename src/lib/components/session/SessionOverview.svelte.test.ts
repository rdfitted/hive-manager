import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import type { AgentInfo, Session } from '$lib/stores/sessions';
import { terminalLifecycle } from './testing/terminalLifecycle';

const fixture = vi.hoisted(() => ({
  setSessions: undefined as ((sessions: Session[]) => void) | undefined,
  setActive: undefined as ((session: Session | null) => void) | undefined,
  setHeartbeats: undefined as ((agents: Record<string, { summary: string; timestamp: string }>) => void) | undefined,
  invoke: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('phosphor-svelte', () => ({
  ArrowsIn: () => {}, ArrowsOut: () => {}, Check: () => {}, Circle: () => {},
  Hourglass: () => {}, Plus: () => {}, X: () => {},
}));
vi.mock('./SessionHeader.svelte', () => ({ default: () => {} }));

vi.mock('$lib/stores/sessions', async () => {
  const { writable, derived } = await import('svelte/store');
  const state = writable({ sessions: [] as Session[] });
  const activeSession = writable<Session | null>(null);
  fixture.setSessions = (sessions) => state.set({ sessions });
  fixture.setActive = activeSession.set;
  return {
    sessions: { subscribe: state.subscribe }, activeSession,
    activeAgents: derived(activeSession, (session) => session?.agents ?? []),
    serdeEnumVariantName: (value: unknown) => typeof value === 'string' ? value
      : value && typeof value === 'object' ? Object.keys(value)[0] : undefined,
  };
});
vi.mock('$lib/stores/cells', async () => {
  const { writable } = await import('svelte/store');
  return { cells: {
    subscribe: writable({ cells: {}, sessionNotFound: false }).subscribe,
    fetchCells: vi.fn().mockResolvedValue(undefined), setExternalRefreshHandler: vi.fn(),
  } };
});
vi.mock('$lib/stores/agents', () => ({ agents: { fetchAgents: vi.fn().mockResolvedValue(undefined) } }));
vi.mock('$lib/stores/events', async () => {
  const { writable } = await import('svelte/store');
  return { events: {
    subscribe: writable({ events: [] }).subscribe, connect: vi.fn(), disconnect: vi.fn(),
  } };
});
vi.mock('$lib/stores/conversations', async () => {
  const { writable } = await import('svelte/store');
  const state = writable({
    agents: {} as Record<string, { summary: string; timestamp: string }>,
    staleAgents: new Set<string>(),
  });
  fixture.setHeartbeats = (agents) => state.set({ agents, staleAgents: new Set() });
  return {
    conversationStore: { pollMessages: vi.fn().mockResolvedValue(undefined) },
    heartbeatStore: { subscribe: state.subscribe, loadHeartbeats: vi.fn().mockResolvedValue(undefined) },
  };
});
vi.mock('../Terminal.svelte', async () => {
  return { default: (await import('./testing/CountingTerminal.svelte')).default };
});

import SessionOverview from './SessionOverview.svelte';
import TerminalGrid from '../TerminalGrid.svelte';
import { layout } from '$lib/stores/layout';
import { ui } from '$lib/stores/ui';
import { scratchTerminals } from '$lib/stores/scratchTerminals';
import { terminalPool } from '$lib/stores/terminalPool';

function session(id = 'session-a', count = 3): Session {
  return {
    id, session_type: { Hive: { worker_count: count - 1 } }, state: 'Running',
    project_path: 'D:/project', worktree_path: 'D:/project/cell', created_at: '2026-10-21T12:00:00Z',
    agents: Array.from({ length: count }, (_, index): AgentInfo => ({
      id: `${id}-agent-${index}`, role: index === 0 ? 'Queen' : { Worker: { index, parent: null } },
      status: 'Running', parent_id: null,
      config: { cli: 'codex', flags: [], label: `Agent ${index}` },
    })),
  };
}

function publish(active: Session, all: Session[] = [active]) {
  fixture.setSessions!(all);
  fixture.setActive!(active);
}

function visiblePane(container: HTMLElement): HTMLElement {
  const pane = container.querySelector<HTMLElement>(
    '.terminal-panel:not(.hidden) .terminal-item:not(.hidden-by-maximize)',
  );
  if (!pane) throw new Error('Expected a visible terminal pane');
  return pane;
}

beforeEach(() => {
  localStorage.clear();
  terminalLifecycle.created.length = 0;
  terminalLifecycle.destroyed.length = 0;
  terminalLifecycle.terminals.clear();
  terminalLifecycle.trace.length = 0;
  fixture.invoke.mockReset().mockImplementation(async (command: string, args: { id?: string }) => {
    terminalLifecycle.trace.push(`${command}:${args.id}`);
  });
  for (const id of Object.keys(get(scratchTerminals).panesBySession)) scratchTerminals.clearSession(id);
  terminalPool.reset();
  layout.setSessionViewMode('focus');
  layout.setMaximizedTerminalId(null);
  ui.setPersistSession(null);
  ui.setFocusedAgent('session-a-agent-0');
  ui.setSelectedAgent('session-a-agent-0');
  fixture.setHeartbeats!({});
  publish(session());
});

afterEach(() => {
  cleanup();
  terminalPool.reset();
});

describe('Focus/Grid session terminals', () => {
  it('G-no-remount: 20 individually flushed Focus agent switches create and destroy zero instances', async () => {
    const view = render(SessionOverview);
    await tick();
    expect(terminalLifecycle.created).toHaveLength(3);
    expect(terminalLifecycle.destroyed).toHaveLength(0);
    const originalNodes = new Map([...terminalLifecycle.terminals].map(([id, terminal]) => [id, terminal.node]));

    for (let index = 1; index <= 20; index++) {
      const id = `session-a-agent-${index % 3}`;
      // Keep selectedAgentId unchanged: explicit focus from the palette must win too.
      ui.setFocusedAgent(id);
      await tick();
      expect(terminalLifecycle.created).toHaveLength(3);
      expect(terminalLifecycle.destroyed).toHaveLength(0);
      expect(view.container.querySelectorAll('.terminal-item')).toHaveLength(3);
      expect(visiblePane(view.container).textContent).toContain(`scrollback:${id}`);
      expect([...terminalLifecycle.terminals].filter(([, terminal]) => terminal.props.isVisible).map(([key]) => key)).toEqual([id]);
      expect(terminalLifecycle.terminals.get(id)?.props.isFocused).toBe(true);
      for (const [key, node] of originalNodes) expect(terminalLifecycle.terminals.get(key)?.node).toBe(node);
    }
  });

  it('defaults to Focus and puts activity and attention above the terminal after removing the old tabs', async () => {
    const active = session();
    active.agents[0].status = { WaitingForInput: 'injected_delivery_failed:nonce:123' };
    publish(active);
    fixture.setHeartbeats!({ [active.agents[0].id]: { summary: 'Checking the terminal pool', timestamp: new Date().toISOString() } });
    const view = render(SessionOverview);
    await tick();
    expect(view.getByRole('button', { name: 'Focus' }).getAttribute('aria-pressed')).toBe('true');
    expect(view.queryByRole('tablist', { name: 'Session view' })).toBeNull();
    expect(view.queryByText('Observability')).toBeNull();
    expect(view.queryByText('Artifacts')).toBeNull();
    const pane = visiblePane(view.container);
    const info = pane.querySelector('.focused-agent-info')!;
    expect(info.textContent).toContain('Checking the terminal pool');
    expect(within(pane).getByRole('region', { name: 'Agent needs attention' })).toBeTruthy();
    expect(info.nextElementSibling?.classList.contains('terminal-container')).toBe(true);
  });

  it('changes Focus/Grid with one click and keeps every existing terminal instance', async () => {
    const view = render(SessionOverview);
    await tick();
    await fireEvent.click(view.getByRole('button', { name: 'Grid' }));
    expect(get(layout).sessionViewMode).toBe('grid');
    expect(view.container.querySelectorAll('.terminal-item:not(.hidden-by-maximize)')).toHaveLength(3);
    expect(view.container.querySelector('.focused-agent-info')).toBeNull();
    await fireEvent.click(view.getByRole('button', { name: 'Focus' }));
    expect(get(layout).sessionViewMode).toBe('focus');
    expect(view.container.querySelectorAll('.terminal-item:not(.hidden-by-maximize)')).toHaveLength(1);
    expect(terminalLifecycle.created).toHaveLength(3);
    expect(terminalLifecycle.destroyed).toHaveLength(0);
    // Positive teardown control proves the counting mock observes component destruction.
    cleanup();
    expect(terminalLifecycle.destroyed).toHaveLength(3);
  });

  it.each(['focus', 'grid'] as const)('opens and closes scratch shells in %s after listeners are ready', async (mode) => {
    layout.setSessionViewMode(mode);
    const view = render(SessionOverview);
    await tick();
    await fireEvent.click(view.getByRole('button', { name: 'Open Terminal' }));
    await tick();
    const scratch = get(scratchTerminals).panesBySession['session-a'][0];
    expect(scratch).toBeTruthy();
    expect(fixture.invoke).toHaveBeenCalledWith('create_pty', expect.objectContaining({
      id: scratch.id, cwd: 'D:/project/cell', role: 'scratch_shell', sessionId: 'session-a',
    }));
    expect(terminalLifecycle.trace.indexOf(`ready:${scratch.id}`)).toBeLessThan(terminalLifecycle.trace.indexOf(`create_pty:${scratch.id}`));
    expect(terminalLifecycle.terminals.get(scratch.id)?.props.isVisible).toBe(true);
    const pane = terminalLifecycle.terminals.get(scratch.id)!.node.closest<HTMLElement>('.terminal-item')!;
    expect(pane.querySelector('.focused-agent-info')).toBeNull();
    await fireEvent.click(within(pane).getByRole('button', { name: `Close ${scratch.title}` }));
    await tick();
    expect(fixture.invoke).toHaveBeenCalledWith('kill_pty', { id: scratch.id });
    expect(terminalLifecycle.destroyed).toEqual([scratch.id]);
    expect(terminalLifecycle.created).toHaveLength(4);
  });

  it('keeps an existing scratch shell reachable from the Focus picker after switching agents', async () => {
    const view = render(SessionOverview);
    await tick();
    await fireEvent.click(view.getByRole('button', { name: 'Open Terminal' }));
    await tick();
    const scratch = get(scratchTerminals).panesBySession['session-a'][0];
    const scratchNode = terminalLifecycle.terminals.get(scratch.id)!.node;
    ui.setFocusedAgent('session-a-agent-1');
    await tick();
    expect(visiblePane(view.container).textContent).toContain('scrollback:session-a-agent-1');
    await fireEvent.change(view.getByRole('combobox', { name: 'Focused terminal' }), { target: { value: scratch.id } });
    expect(visiblePane(view.container).textContent).toContain(`scrollback:${scratch.id}`);
    expect(terminalLifecycle.terminals.get(scratch.id)?.node).toBe(scratchNode);
    expect(terminalLifecycle.created).toHaveLength(4);
    expect(terminalLifecycle.destroyed).toHaveLength(0);
  });

  it('preserves pooled session grids and their scrollback across session changes', async () => {
    const first = session();
    const second = session('session-b', 2);
    publish(first, [first, second]);
    const view = render(SessionOverview);
    await tick();
    const original = terminalLifecycle.terminals.get(first.agents[0].id)!.node;
    fixture.setActive!(second);
    ui.setFocusedAgent(second.agents[0].id);
    await tick();
    expect(terminalLifecycle.created).toHaveLength(5);
    expect(visiblePane(view.container).textContent).toContain(`scrollback:${second.agents[0].id}`);
    expect(terminalLifecycle.terminals.get(first.agents[0].id)?.props.isVisible).toBe(false);
    fixture.setActive!(first);
    ui.setFocusedAgent(first.agents[0].id);
    await tick();
    expect(terminalLifecycle.terminals.get(first.agents[0].id)?.node).toBe(original);
    expect(terminalLifecycle.created).toHaveLength(5);
    expect(terminalLifecycle.destroyed).toHaveLength(0);
    expect(visiblePane(view.container).textContent).toContain(`scrollback:${first.agents[0].id}`);
  });

  it('keeps the Grid maximize guard local to each pooled session', async () => {
    layout.setSessionViewMode('grid');
    layout.setMaximizedTerminalId('another-session-agent');
    const view = render(TerminalGrid, { session: session(), focusedAgentId: null, visible: false, onSelect: vi.fn() });
    await tick();
    expect(view.container.querySelectorAll('.hidden-by-maximize')).toHaveLength(0);
    expect(view.container.querySelector('.has-maximized')).toBeNull();
    expect(get(layout).maximizedTerminalId).toBe('another-session-agent');
  });
});
