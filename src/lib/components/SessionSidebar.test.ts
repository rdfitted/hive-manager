// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { get, type Writable } from 'svelte/store';
import type { AgentInfo, Session } from '$lib/stores/sessions';

const launchMocks = vi.hoisted(() => ({ goto: vi.fn().mockResolvedValue(undefined), invoke: vi.fn().mockResolvedValue([]) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: launchMocks.invoke }));
vi.mock('$app/navigation', () => ({ goto: launchMocks.goto }));
vi.mock('$app/stores', async () => {
  const { writable } = await import('svelte/store');
  return { page: writable({ url: new URL('http://localhost/') }) };
});
vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    sessions: { ...writable({ sessions: [] }), setActiveSession: vi.fn() },
    activeSession: writable(null),
    activeAgents: writable([]),
    serdeEnumVariantName: (value: unknown) => typeof value === 'string' ? value : value && typeof value === 'object' ? Object.keys(value)[0] : undefined,
  };
});
vi.mock('./LaunchDialog.svelte', () => ({ default: () => {} }));
vi.mock('$lib/stores/conversations', async () => ({ heartbeatStore: { ...(await import('svelte/store')).writable({ agents: {}, staleAgents: new Set(), stalledAgents: new Set() }), loadHeartbeats: vi.fn().mockResolvedValue(undefined) } }));
vi.mock('./QueenControls.svelte', () => ({ default: () => {} }));
vi.mock('./ResumeConfirmModal.svelte', () => ({ default: () => {} }));

import SessionSidebar from './SessionSidebar.svelte';
import { layout } from '$lib/stores/layout';
import { sessions, activeSession, activeAgents } from '$lib/stores/sessions';
import { ui } from '$lib/stores/ui';
import { launch } from '$lib/stores/launch';
import { page } from '$app/stores';

beforeEach(() => {
  vi.clearAllMocks();
  launch.close();
  (page as unknown as Writable<unknown>).set({ url: new URL('http://localhost/') });
  launchMocks.invoke.mockResolvedValue([]);
  (sessions as unknown as Writable<unknown>).set({ sessions: [] });
  (activeSession as Writable<Session | null>).set(null);
  (activeAgents as Writable<AgentInfo[]>).set([]);
  ui.setFocusedAgent(null);
  if (get(layout).leftCollapsed) layout.toggleLeft();
});

describe('SessionSidebar launch entries', () => {
  it('New Session opens a fresh request without a mode or a second dialog', async () => {
    const view = render(SessionSidebar);
    const previousId = get(launch).requestId;
    await fireEvent.click(view.getByRole('button', { name: 'New Session' }));
    expect(get(launch)).toMatchObject({ landingOpen: true, requestedMode: undefined });
    expect(get(launch).requestId).toBeGreaterThan(previousId);
    expect(sessions.setActiveSession).not.toHaveBeenCalled();
    expect(view.queryByRole('dialog')).toBeNull();
  });

  it('preserves the running session when opening the inline landing', async () => {
    const active = { id: 'active', project_path: 'D:/project', state: 'Running', session_type: { Hive: {} }, agents: [] } as unknown as Session;
    (activeSession as Writable<Session | null>).set(active);
    const view = render(SessionSidebar);
    await fireEvent.click(view.getByRole('button', { name: 'New Session' }));
    expect(get(activeSession)).toBe(active);
    expect(sessions.setActiveSession).not.toHaveBeenCalled();
    expect(get(launch).landingOpen).toBe(true);
  });

  it('awaits navigation to / before opening New Session off-route', async () => {
    (page as unknown as Writable<unknown>).set({ url: new URL('http://localhost/knowledge') });
    let finish!: () => void;
    launchMocks.goto.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const view = render(SessionSidebar);
    const previousId = get(launch).requestId;
    await fireEvent.click(view.getByRole('button', { name: 'New Session' }));
    expect(launchMocks.goto).toHaveBeenCalledWith('/');
    expect(get(launch).requestId).toBe(previousId);
    finish();
    await waitFor(() => expect(get(launch).requestId).toBeGreaterThan(previousId));
  });

  it.each(['hive', 'fusion', 'debate'] as const)('routes the %s start action off-route and reapplies repeated requests', async (action) => {
    (page as unknown as Writable<unknown>).set({ url: new URL('http://localhost/dashboard') });
    const view = render(SessionSidebar, { props: { onLaunchHiveV2: vi.fn(), startAction: { id: 1, action } } });
    await waitFor(() => expect(get(launch).requestedMode).toBe(action));
    expect(launchMocks.goto).toHaveBeenCalledWith('/');
    const previousId = get(launch).requestId;
    await view.rerender({ onLaunchHiveV2: vi.fn(), startAction: { id: 2, action } });
    await waitFor(() => expect(get(launch).requestId).toBeGreaterThan(previousId));
  });

  it('selects the most recently active persisted project as the default', async () => {
    launchMocks.invoke.mockImplementation(async (command: string) => command === 'get_current_directory' ? 'D:/cwd' : [
      { id: 'older', project_path: 'D:/older', created_at: '2026-01-01', last_activity_at: '2026-01-03' },
      { id: 'latest', project_path: 'D:/latest', created_at: '2026-01-02', last_activity_at: '2026-10-22' },
    ]);
    render(SessionSidebar);
    await waitFor(() => expect(get(launch).defaultProjectPath).toBe('D:/latest'));
  });

  it('falls back to the current directory when no persisted session exists', async () => {
    launchMocks.invoke.mockImplementation(async (command: string) => command === 'get_current_directory' ? 'D:/cwd' : []);
    render(SessionSidebar);
    await waitFor(() => expect(get(launch).defaultProjectPath).toBe('D:/cwd'));
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('SessionSidebar rail toggle', () => {
  it('has a visible button that collapses and expands the sidebar', async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32');
    const view = render(SessionSidebar);
    const toggle = view.getByRole('button', { name: 'Collapse sidebar (Ctrl+B)' });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(toggle.querySelector('svg')).toBeTruthy();

    await fireEvent.click(toggle);
    expect(get(layout).leftCollapsed).toBe(true);
    expect(toggle.getAttribute('aria-label')).toBe('Expand sidebar (Ctrl+B)');

    await fireEvent.click(toggle);
    expect(get(layout).leftCollapsed).toBe(false);
  });

  it('labels the shortcut with ⌘ on macOS', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel');
    const view = render(SessionSidebar);
    expect(view.getByRole('button', { name: 'Collapse sidebar (⌘B)' })).toBeTruthy();
  });
});


describe('sidebar session and agent attention', () => {
  const agent = (id: string, reason: string): AgentInfo => ({
    id, role: 'Queen', status: { WaitingForInput: reason }, config: { cli: 'codex', flags: [], label: id }, parent_id: null,
  });
  const session = (id: string, agents: AgentInfo[]): Session => ({
    id, name: id, project_path: 'D:/project', created_at: '2026-10-21T12:00:00Z',
    state: 'Running', session_type: { Hive: { worker_count: agents.length } }, agents,
  } as Session);
  function load(active: Session, inactive: Session) {
    (sessions as unknown as Writable<unknown>).set({ sessions: [active, inactive] });
    (activeSession as Writable<Session | null>).set(active);
    (activeAgents as Writable<AgentInfo[]>).set(active.agents);
    if (get(layout).agentsCollapsed) layout.toggleSection('agentsCollapsed');
    if (get(layout).sessionsCollapsed) layout.toggleSection('sessionsCollapsed');
  }
  it('nests the tree under the active session, shows failed-delivery badges, and focuses by badge', async () => {
    const active = session('Active hive', [agent('Failed worker', 'injected_delivery_failed:transcript:1')]);
    const inactive = session('Other hive', [agent('Other failed', 'injected_delivery_failed:transcript:2')]);
    load(active, inactive); const view = render(SessionSidebar);
    const tree = view.getByRole('tree', { name: 'Agent hierarchy' });
    expect(tree.closest('li')?.textContent).toContain('Active hive');
    expect(view.getAllByRole('tree')).toHaveLength(1);
    expect(view.getByRole('button', { name: '1 need attention in Other hive' })).toBeTruthy();
    expect(view.getByRole('button', { name: '1 need attention in Active hive' })).toBeTruthy();
    await fireEvent.click(view.getByRole('button', { name: 'Needs you: Delivery failed — Failed worker' }));
    expect(get(ui).focusedAgentId).toBe('Failed worker');
    await fireEvent.click(view.getByRole('button', { name: '1 need attention in Other hive' }));
    expect(sessions.setActiveSession).toHaveBeenCalledWith('Other hive');
    expect(get(ui).focusedAgentId).toBe('Other failed');
    expect(tree.textContent).not.toContain('⏳');
  });
  it('keeps the selected session tree reachable after completion or failure', async () => {
    const active = session('Finished hive', [agent('Worker', 'awaiting_injected_turn:1')]);
    load(active, session('Other hive', []));
    const view = render(SessionSidebar);
    for (const state of ['Completed', { Failed: 'worker crashed' }] as Session['state'][]) {
      const finished = { ...active, state };
      (sessions as unknown as Writable<unknown>).set({ sessions: [finished] });
      (activeSession as Writable<Session | null>).set(finished);
      await import('svelte').then(({ tick }) => tick());
      expect(view.getByRole('tree', { name: 'Agent hierarchy' }).closest('li')?.textContent).toContain('Finished hive');
    }
  });
  it('shows no attention for awaiting an injected turn on either row or session', () => {
    load(session('Active hive', [agent('Waiting worker', 'awaiting_injected_turn:1')]), session('Other hive', []));
    const view = render(SessionSidebar);
    expect(view.queryByRole('button', { name: /Needs you:/ })).toBeNull();
    expect(view.queryByRole('button', { name: /need attention in/ })).toBeNull();
    expect(view.getByRole('img', { name: 'WaitingForInput' })).toBeTruthy();
  });
});
