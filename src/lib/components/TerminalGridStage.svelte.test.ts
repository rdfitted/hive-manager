import { cleanup, fireEvent, render, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { heartbeatStore } from '$lib/stores/conversations';
import { layout } from '$lib/stores/layout';
import { scratchTerminals } from '$lib/stores/scratchTerminals';
import type { AgentInfo, Session } from '$lib/stores/sessions';
import TerminalGrid from './TerminalGrid.svelte';
import { terminalLifecycle } from './session/testing/terminalLifecycle';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('phosphor-svelte', () => ({
  Bell: () => {}, ChatCircle: () => {}, ListBullets: () => {}, SquareSplitVertical: () => {}, TerminalWindow: () => {}, Timer: () => {},
  ArrowUpRight: () => {}, ArrowUp: () => {}, ArrowsIn: () => {}, ArrowsOut: () => {}, Check: () => {},
  Circle: () => {}, Hourglass: () => {}, Plus: () => {}, X: () => {},
}));
vi.mock('./Terminal.svelte', async () => ({
  default: (await import('./session/testing/CountingTerminal.svelte')).default,
}));

const { readFileSync } = await vi.importActual<{ readFileSync(path: string, encoding: 'utf8'): string }>('node:fs');

const roster: Session = {
  id: 'stage', session_type: { Hive: { worker_count: 5 } }, state: 'Running',
  project_path: 'D:/fixture', worktree_path: 'D:/fixture/cell', created_at: '2026-01-01T00:00:00Z',
  agents: Array.from({ length: 6 }, (_, index): AgentInfo => ({
    id: index === 0 ? 'queen' : `worker-${index}`,
    role: index === 0 ? 'Queen' : { Worker: { index, parent: null } },
    parent_id: null,
    config: { cli: 'codex', flags: [], label: index === 0 ? 'Queen' : `Worker ${index}` },
    status: index === 5 ? 'Completed' : 'Running',
    work_state: index === 4 ? 'idle' : undefined,
  })),
};

function renderStage() {
  const onSelect = vi.fn();
  const view = render(TerminalGrid, { session: roster, focusedAgentId: 'queen', onSelect });
  const stage = view.container.querySelector<HTMLElement>('.terminal-grid')!;
  const workers = stage.querySelector<HTMLElement>('.worker-viewport')!;
  const card = (id: string) => stage.querySelector<HTMLElement>(`[data-agent-id="${id}"]`)!;
  return { ...view, stage, workers, card, onSelect };
}

function overflowAxes(element: HTMLElement) {
  const style = getComputedStyle(element);
  const [horizontal, vertical = horizontal] = style.overflow.split(/\s+/);
  return { x: style.overflowX || horizontal, y: style.overflowY || vertical };
}

let stageCss: HTMLStyleElement;

beforeEach(() => {
  // Vite extracts Svelte CSS. Attach the production rules for computed-style assertions in jsdom.
  const source = readFileSync('src/lib/components/TerminalGrid.svelte', 'utf8');
  stageCss = document.createElement('style');
  stageCss.textContent = source.slice(source.indexOf('<style>') + '<style>'.length, source.indexOf('</style>'));
  document.head.append(stageCss);
  layout.setSessionViewMode('grid');
  layout.setMaximizedTerminalId(null);
  scratchTerminals.clearSession(roster.id);
  terminalLifecycle.created.length = 0;
  terminalLifecycle.destroyed.length = 0;
  terminalLifecycle.terminals.clear();
  terminalLifecycle.trace.length = 0;
});

afterEach(() => {
  stageCss.remove();
  cleanup();
  scratchTerminals.clearSession(roster.id);
});

describe('vertical terminal stage', () => {
  it('offers Open Terminal as a named icon with a focus tooltip', async () => {
    const view = renderStage();
    const button = view.getByRole('button', { name: 'Open Terminal' });
    expect(button.textContent?.trim()).toBe('');
    button.focus();
    expect((await view.findByRole('tooltip')).textContent).toContain('Open Terminal');
  });

  it('G-vertical-stack: one Queen outside the y-scrolling viewport of five workers', () => {
    const { stage, workers, card } = renderStage();
    const queen = card('queen');
    expect(queen.closest('.queen-area')?.parentElement).toBe(stage);
    expect(workers.contains(queen)).toBe(false);
    expect(workers.querySelectorAll(':scope > .terminal-item')).toHaveLength(5);
    expect(stage.querySelectorAll('.terminal-item')).toHaveLength(6);

    // jsdom can verify the authored scroll policy. The render harness checks real scroll dimensions.
    expect(overflowAxes(stage)).toEqual({ x: 'clip', y: 'clip' });
    expect(overflowAxes(workers)).toEqual({ x: 'hidden', y: 'auto' });
    expect(getComputedStyle(workers).flexDirection).toBe('column');
    expect(getComputedStyle(stage).gridTemplateColumns).toMatch(/minmax\(0, 1[.]42fr\) minmax\(0, 1fr\)/);
  });

  it('expands a worker when current work and newer input supersede its idle heartbeat', async () => {
    const workerId = 'worker-1';
    const time = Date.now();
    const lastActivity = new Date(time - 10_000).toISOString();
    const newerInput = new Date(time - 5_000).toISOString();
    const fetchActive = vi.fn();
    vi.stubGlobal('fetch', fetchActive);
    const updateWorker = async (workState?: string, latestInput?: string) => {
      const agents = [{ id: workerId, status: 'idle', last_activity: lastActivity,
        work_state: workState, latest_input_at: latestInput }];
      fetchActive.mockResolvedValue({ ok: true, json: async () => ({ sessions: [{ id: roster.id, agents }] }) });
      await heartbeatStore.loadHeartbeats(roster.id);
      await tick();
    };

    try {
      await updateWorker('working');
      const { card } = renderStage();
      const worker = within(card(workerId));
      const terminal = () => terminalLifecycle.terminals.get(workerId)!;
      const node = terminal().node;
      expect(terminal().props.isVisible).toBe(false);

      for (const [state, input] of [
        ['working', lastActivity], [undefined, newerInput], ['finished', newerInput], ['retired', newerInput],
      ]) {
        await updateWorker(state, input);
        expect(terminal().props.isVisible).toBe(false);
      }

      await updateWorker('working', newerInput);
      expect(terminal().props.isVisible).toBe(true);
      expect(terminal().node).toBe(node);
      await fireEvent.click(worker.getByRole('button', { name: 'Collapse Worker 1' }));
      await updateWorker('working', new Date(time).toISOString());
      expect(terminal().props.isVisible).toBe(false);
      expect(terminal().node).toBe(node);
      expect(terminalLifecycle.created.filter(id => id === workerId)).toHaveLength(1);
      expect(terminalLifecycle.destroyed).not.toContain(workerId);
    } finally {
      fetchActive.mockResolvedValue({ ok: true, json: async () => ({ sessions: [] }) });
      await heartbeatStore.loadHeartbeats(null);
      vi.unstubAllGlobals();
    }
  });

  it('keeps each terminal alive through collapse, expansion, attention, focus, maximise and scratch actions', async () => {
    const view = renderStage();
    await tick();
    const { card, workers } = view;
    const quiet = within(card('worker-4'));
    const nodes = new Map([...terminalLifecycle.terminals].map(([id, terminal]) => [id, terminal.node]));
    expect([...nodes.keys()]).toEqual(roster.agents.map(agent => agent.id));
    const assertStable = () => {
      expect(terminalLifecycle.created.filter(id => !id.startsWith('scratch:')).sort()).toEqual([...nodes.keys()].sort());
      expect(terminalLifecycle.destroyed.filter(id => !id.startsWith('scratch:'))).toEqual([]);
      for (const [id, node] of nodes) {
        expect(terminalLifecycle.terminals.get(id)?.node).toBe(node);
        expect(node.textContent).toBe(`scrollback:${id}`);
      }
    };
    const visible = (id: string) => terminalLifecycle.terminals.get(id)?.props.isVisible;

    expect(visible('worker-4')).toBe(false);
    expect(visible('worker-5')).toBe(false);
    await fireEvent.click(quiet.getByRole('button', { name: 'Expand Worker 4' }));
    expect(visible('worker-4')).toBe(true);
    await fireEvent.click(quiet.getByRole('button', { name: 'Collapse Worker 4' }));
    expect(visible('worker-4')).toBe(false);
    assertStable();

    const attention: Session = { ...roster, agents: roster.agents.map(agent => agent.id === 'worker-4'
      ? { ...agent, status: { Error: 'fixture attention' } } : agent) };
    await view.rerender({ session: attention });
    expect(getComputedStyle(card('worker-4')).order).toBe('-1');
    expect(card('worker-4').parentElement).toBe(workers);
    expect(quiet.getByRole('region', { name: 'Agent needs attention' })).toBeTruthy();
    expect(visible('worker-4')).toBe(true);
    assertStable();
    await view.rerender({ session: roster });
    expect(visible('worker-4')).toBe(false);

    const queen = card('queen');
    await fireEvent.click(within(queen).getByRole('button', { name: /^Terminal$/ }));
    expect(queen.querySelector<HTMLElement>('.agent-pane-body')?.dataset.mode).toBe('terminal');
    await fireEvent.click(quiet.getByRole('button', { name: 'Message Worker 4' }));
    expect(queen.querySelector<HTMLElement>('.agent-pane-body')?.dataset.mode).toBe('chat');
    expect(within(queen).getByRole('button', { name: 'Command target: @Worker 4' })).toBeTruthy();
    expect(view.onSelect).toHaveBeenLastCalledWith('worker-4');
    expect(document.activeElement).toBe(view.container.querySelector('.queen-column .chat-composer [data-composer]'));

    await view.rerender({ focusedAgentId: 'worker-4' });
    layout.setSessionViewMode('focus');
    await tick();
    expect((view.getByRole('combobox', { name: 'Focused terminal' }) as HTMLSelectElement).value).toBe('worker-4');
    expect(card('worker-4').classList.contains('maximized')).toBe(true);
    expect(visible('worker-4')).toBe(true);
    expect(visible('queen')).toBe(false);
    layout.setSessionViewMode('grid');
    await tick();
    expect(visible('worker-4')).toBe(false);
    await fireEvent.click(quiet.getByRole('button', { name: 'Maximize Worker 4' }));
    expect(visible('worker-4')).toBe(true);
    expect(visible('queen')).toBe(false);
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(visible('worker-4')).toBe(false);
    expect(visible('queen')).toBe(true);
    assertStable();

    const scratch = scratchTerminals.add(roster.id, 'D:/fixture', 'login');
    await tick();
    expect(terminalLifecycle.created.filter(id => id === scratch.id)).toHaveLength(1);
    expect(workers.contains(terminalLifecycle.terminals.get(scratch.id)!.node)).toBe(true);
    expect(visible(scratch.id)).toBe(true);
    await fireEvent.click(view.getByRole('button', { name: `Close ${scratch.title}` }));
    expect(terminalLifecycle.destroyed).toEqual([scratch.id]);
    assertStable();
  });
});
