// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import { get, type Writable } from 'svelte/store';
import { tick } from 'svelte';
import treeItemSource from './TreeItem.svelte?raw';
import type { AgentInfo, Session } from '$lib/stores/sessions';
import type { WorkGraphResponse } from '$lib/workgraph/types';

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: tauri.invoke }));
vi.mock('$app/navigation', () => ({ goto: vi.fn().mockResolvedValue(undefined) }));
vi.mock('$app/stores', async () => ({ page: (await import('svelte/store')).writable({ url: new URL('http://localhost/') }) }));
vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    sessions: { ...writable({ sessions: [] }), setActiveSession: vi.fn() },
    activeSession: writable(null), activeAgents: writable([]),
    serdeEnumVariantName: (value: unknown) => typeof value === 'string' ? value : value && typeof value === 'object' ? Object.keys(value)[0] : undefined,
  };
});
vi.mock('$lib/stores/conversations', async () => ({ heartbeatStore: {
  ...(await import('svelte/store')).writable({ agents: {}, staleAgents: new Set(), stalledAgents: new Set() }),
  loadHeartbeats: vi.fn().mockResolvedValue(undefined),
} }));
vi.mock('./QueenControls.svelte', () => ({ default: () => {} }));
vi.mock('./ResumeConfirmModal.svelte', () => ({ default: () => {} }));

import SessionSidebar from './SessionSidebar.svelte';
import { sessions, activeAgents, activeSession } from '$lib/stores/sessions';
import { layout } from '$lib/stores/layout';
import { ui } from '$lib/stores/ui';
import { DEPENDENCY_POLL_MS } from '$lib/stores/agentDependencies';

const agent = (id: string, parent_id: string | null = null): AgentInfo => ({
  id, parent_id, role: parent_id ? { Worker: { index: 1, parent: parent_id } } : 'Queen', status: 'Running',
  config: { cli: 'codex', flags: [], label: id },
});
let agents: AgentInfo[];
function graph(completed = false): WorkGraphResponse {
  return {
    nodes: [
      { id: 'base', title: 'Foundation', status: completed ? 'completed' : 'running', progress: { agent_id: 'foundation' } },
      { id: 'work', title: 'Frontend', status: 'pending', progress: { agent_id: 'worker' } },
      { id: 'future', title: 'Unassigned review', status: 'pending', progress: { agent_id: null } },
      { id: 'info', title: 'Informational only', status: 'running', progress: { agent_id: 'queen' } },
    ], edges: [
      { source: 'base', target: 'work', kind: 'depends_on' },
      { source: 'work', target: 'future', kind: 'depends_on' },
      { source: 'info', target: 'work', kind: 'informs' },
      { source: 'work', target: 'info', kind: 'touches' },
    ], status_by_node: {},
  } as WorkGraphResponse;
}
function load() {
  agents = [agent('queen'), agent('worker', 'queen'), agent('foundation')];
  const session = { id: 'active', name: 'Active hive', project_path: 'D:/project', state: 'Running',
    created_at: '2026-10-31', session_type: { Hive: { worker_count: 3 } }, agents } as Session;
  (sessions as unknown as Writable<unknown>).set({ sessions: [session] });
  (activeSession as Writable<Session | null>).set(session);
  (activeAgents as Writable<AgentInfo[]>).set(agents);
}
const row = (view: { container: HTMLElement }, id: string) => view.container.querySelector<HTMLElement>(`[data-agent-id="${id}"]`)!;

beforeEach(() => {
  tauri.invoke.mockResolvedValue([]);
  for (const section of ['sessionsCollapsed', 'agentsCollapsed'] as const) if (get(layout)[section]) layout.toggleSection(section);
  if (get(layout).leftCollapsed) layout.toggleLeft();
  ui.setFocusedAgent('queen'); load();
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });

describe('dependency rows and G3 focus', () => {
  it('G3: refresh and status changes leave focusedAgentId alone; only a click moves it', async () => {
    // Keep Queen selected across both update phases: focus-on-refresh and
    // focus-on-Error mutants must each fail before the explicit worker click.
    vi.useFakeTimers();
    const fetcher = vi.fn().mockResolvedValue({ ok: true, json: async () => graph() });
    vi.stubGlobal('fetch', fetcher);
    const focus = vi.spyOn(ui, 'setFocusedAgent');
    const view = render(SessionSidebar);
    for (let i = 0; i < 10; i++) await Promise.resolve(); await tick();
    expect(within(row(view, 'worker')).getByRole('button', { name: 'Focus agent for Foundation' })).toBeTruthy();
    expect(get(ui).focusedAgentId).toBe('queen');
    expect(focus).not.toHaveBeenCalled();

    fetcher.mockResolvedValue({ ok: true, json: async () => graph(true) });
    await vi.advanceTimersByTimeAsync(DEPENDENCY_POLL_MS); await tick();
    expect(get(ui).focusedAgentId).toBe('queen');
    expect(focus).not.toHaveBeenCalled();
    (activeAgents as Writable<AgentInfo[]>).set(agents.map((item) => item.id === 'worker' ? { ...item, status: { Error: 'crash' } } : item));
    await tick();
    expect(within(row(view, 'worker')).queryByText('Waiting on:')).toBeNull();
    expect(within(row(view, 'worker')).getByText('Error')).toBeTruthy();
    expect(get(ui).focusedAgentId).toBe('queen');
    expect(focus).not.toHaveBeenCalled();
    await fireEvent.click(row(view, 'worker'));
    expect(get(ui).focusedAgentId).toBe('worker');
    expect(focus).toHaveBeenCalledTimes(1);
    expect(focus).toHaveBeenCalledWith('worker');
  });

  it('clicks the referenced agent once, renders null-agent refs as text and excludes informational edges', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => graph() }));
    const view = render(SessionSidebar);
    await waitFor(() => expect(within(row(view, 'worker')).getByText('Waiting on:')).toBeTruthy());
    const work = within(row(view, 'worker'));
    expect(work.getByText('Unassigned review').tagName).toBe('SPAN');
    expect(work.queryByRole('button', { name: /Unassigned review/ })).toBeNull();
    expect(work.queryByText('Informational only')).toBeNull();
    const focus = vi.spyOn(ui, 'setFocusedAgent');
    await fireEvent.click(work.getByRole('button', { name: 'Focus agent for Foundation' }));
    expect(get(ui).focusedAgentId).toBe('foundation');
    expect(focus).toHaveBeenCalledTimes(1);
    expect(focus).toHaveBeenCalledWith('foundation');
  });

  it.each([404, 500, 'offline'])('keeps Queen-to-worker delegation and visible status without an error when graph is %s', async (status) => {
    const fetcher = status === 'offline' ? vi.fn().mockRejectedValue(new Error('offline')) : vi.fn().mockResolvedValue({ ok: false, status });
    vi.stubGlobal('fetch', fetcher);
    const view = render(SessionSidebar);
    await waitFor(() => expect(fetcher).toHaveBeenCalled());
    expect(row(view, 'worker').closest('[role="group"]')?.parentElement?.querySelector('[data-agent-id="queen"]')).toBeTruthy();
    expect(within(row(view, 'worker')).getByText('Running')).toBeTruthy();
    expect(view.queryByRole('alert')).toBeNull();
    expect(view.queryByText(/offline|HTTP|graph unavailable/i)).toBeNull();
  });

  it('navigates visible tree rows with arrows without selecting or intercepting ref activation', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => graph() }));
    const view = render(SessionSidebar);
    await waitFor(() => expect(within(row(view, 'worker')).getByText('Waiting on:')).toBeTruthy());
    const queen = row(view, 'queen'); queen.focus();
    await fireEvent.keyDown(queen, { key: 'ArrowRight' });
    expect(document.activeElement).toBe(row(view, 'worker'));
    await fireEvent.keyDown(row(view, 'worker'), { key: 'ArrowLeft' });
    expect(document.activeElement).toBe(queen);
    await fireEvent.keyDown(queen, { key: 'ArrowLeft' });
    expect(queen.getAttribute('aria-expanded')).toBe('false');
    expect(row(view, 'worker')).toBeNull();
    await fireEvent.keyDown(queen, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(row(view, 'foundation'));
    await fireEvent.keyDown(row(view, 'foundation'), { key: 'Home' });
    expect(document.activeElement).toBe(queen);
    await fireEvent.keyDown(queen, { key: 'ArrowRight' });
    await fireEvent.keyDown(queen, { key: 'End' });
    expect(document.activeElement).toBe(row(view, 'foundation'));
    await fireEvent.keyDown(row(view, 'foundation'), { key: 'ArrowUp' });
    expect(document.activeElement).toBe(row(view, 'worker'));
    expect(get(ui).focusedAgentId).toBe('queen');
    const ref = within(row(view, 'worker')).getByRole('button', { name: 'Focus agent for Foundation' });
    ref.focus(); await fireEvent.keyDown(ref, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(ref);
    await fireEvent.click(ref);
    expect(get(ui).focusedAgentId).toBe('foundation');
    await fireEvent.keyDown(row(view, 'queen'), { key: 'Enter' });
    expect(get(ui).focusedAgentId).toBe('queen');
  });

  it('retains owning-element system-colour borders and focus outlines for forced colours', () => {
    const forced = treeItemSource.slice(treeItemSource.indexOf('@media (forced-colors: active)'));
    expect(forced).toContain('.status-dot { background: CanvasText; border-color: CanvasText; }');
    expect(forced).toContain('.attention-badge { border: 1px solid ButtonText; }');
    expect(forced).toContain('outline: 2px solid Highlight');
    expect(forced).toContain('.dependency-ref { color: ButtonText; border-color: ButtonText;');
  });

  it('keeps a visible keyboard entry when selection belongs to another session or the navigated agent disappears', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 404 }));
    ui.setFocusedAgent('previous-session');
    const view = render(SessionSidebar); await tick();
    expect(row(view, 'queen').tabIndex).toBe(0);
    row(view, 'foundation').focus();
    await tick();
    expect(row(view, 'foundation').tabIndex).toBe(0);
    (activeAgents as Writable<AgentInfo[]>).set(agents.filter((item) => item.id !== 'foundation'));
    await tick(); await Promise.resolve();
    expect(row(view, 'queen').tabIndex).toBe(0);
    row(view, 'worker').focus(); await tick();
    await fireEvent.click(within(row(view, 'queen')).getByRole('button', { name: 'Collapse' }));
    await tick(); await Promise.resolve();
    expect(row(view, 'queen').tabIndex).toBe(0);
    expect(get(ui).focusedAgentId).toBe('previous-session');
  });
});
