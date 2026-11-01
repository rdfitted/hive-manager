// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import { get, type Writable } from 'svelte/store';
import type { AgentInfo, Session } from '$lib/stores/sessions';

const launchMocks = vi.hoisted(() => ({ goto: vi.fn().mockResolvedValue(undefined), invoke: vi.fn().mockResolvedValue([]), closeSession: vi.fn().mockResolvedValue(undefined) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: launchMocks.invoke }));
vi.mock('$app/navigation', () => ({ goto: launchMocks.goto }));
vi.mock('$app/stores', async () => {
  const { writable } = await import('svelte/store');
  return { page: writable({ url: new URL('http://localhost/') }) };
});
vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    sessions: { ...writable({ sessions: [] }), setActiveSession: vi.fn(), closeSession: launchMocks.closeSession },
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
import TitleBar from './shell/TitleBar.svelte';
import { layout } from '$lib/stores/layout';
import { sessions, activeSession, activeAgents } from '$lib/stores/sessions';
import { ui } from '$lib/stores/ui';
import { launch } from '$lib/stores/launch';
import { page } from '$app/stores';

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 404 }));
  launch.close();
  (page as unknown as Writable<unknown>).set({ url: new URL('http://localhost/') });
  launchMocks.invoke.mockResolvedValue([]);
  launchMocks.closeSession.mockResolvedValue(undefined);
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
  vi.unstubAllGlobals();
});

describe('sidebar toggle moved from rail to TitleBar (D4)', () => {
  it('returns focus to the title-bar toggle after collapsing inside the drawer', async () => {
    const sidebar = render(SessionSidebar);
    const toolbar = render(TitleBar);
    const toggle = toolbar.container.querySelector<HTMLButtonElement>('#title-bar-sidebar-toggle')!;
    const focus = vi.spyOn(toggle, 'focus');
    const close = within(sidebar.container).getByRole('button', { name: /Collapse sidebar/ });
    close.focus();
    await fireEvent.click(close);
    expect(get(layout).leftCollapsed).toBe(true);
    expect(document.activeElement).toBe(toggle);
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
  });

  it('does not steal terminal focus on an external sidebar collapse', async () => {
    render(SessionSidebar);
    render(TitleBar);
    const terminal = document.createElement('textarea');
    document.body.append(terminal);
    terminal.focus();
    layout.toggleLeft();
    await import('svelte').then(({ tick }) => tick());
    expect(document.activeElement).toBe(terminal);
    terminal.remove();
  });

  it('has a visible button that collapses and expands the sidebar', async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32');
    const view = render(TitleBar);
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
    const view = render(TitleBar);
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

describe('sidebar project grouping and header navigation', () => {
  async function clickCloseBackdrop() {
    await new Promise((resolve) => requestAnimationFrame(resolve));
    const backdrop = document.body.querySelector<HTMLElement>('.dialog-backdrop');
    expect(backdrop).not.toBeNull();
    backdrop!.dispatchEvent(new MouseEvent('pointerdown', {
      bubbles: true, cancelable: true, button: 0, clientX: 100, clientY: 100,
    }));
    backdrop!.click();
    await new Promise((resolve) => setTimeout(resolve, 30));
  }
  function session(id: string, project_path: string): Session {
    return { id, name: id, project_path, created_at: '2026-10-31', state: 'Running',
      session_type: { Hive: { worker_count: 0 } }, agents: [] } as Session;
  }
  function load(items: Session[]) {
    (sessions as unknown as Writable<unknown>).set({ sessions: items });
    if (get(layout).sessionsCollapsed) layout.toggleSection('sessionsCollapsed');
  }

  it('groups by full project_path and keeps sessions in the correct project', () => {
    load([session('A1', 'D:/one/project'), session('B1', 'D:/two/project'), session('A2', 'D:/one/project')]);
    const view = render(SessionSidebar);
    expect(view.getAllByRole('heading', { level: 4 }).map((heading) => heading.textContent)).toEqual(['D:/one/project', 'D:/two/project']);
    const one = within(view.getByRole('list', { name: 'Sessions in D:/one/project' }));
    expect(one.getByText('A1')).toBeTruthy(); expect(one.getByText('A2')).toBeTruthy();
    expect(one.queryByText('B1')).toBeNull();
    expect(within(view.getByRole('list', { name: 'Sessions in D:/two/project' })).getByText('B1')).toBeTruthy();
  });

  it('keeps one project flat and excludes inactive sessions in other projects', () => {
    load([session('A1', 'D:/one'), session('A2', 'D:/one'), { ...session('Old', 'D:/two'), state: 'Completed' }]);
    const view = render(SessionSidebar);
    expect(view.queryAllByRole('heading', { level: 4 })).toHaveLength(0);
    expect(within(view.getByRole('list', { name: 'Active sessions' })).getAllByRole('listitem')).toHaveLength(2);
    expect(view.queryByText('Old')).toBeNull();
  });

  it('keeps all three destinations in the sidebar header and removes the rail', async () => {
    const active = session('A', 'D:/one');
    load([active]); (activeSession as Writable<Session | null>).set(active);
    const view = render(SessionSidebar);
    const header = view.container.querySelector('aside header')!;
    const nav = within(header as HTMLElement);
    expect(nav.getByRole('link', { name: 'Sessions' }).getAttribute('href')).toBe('/');
    expect(nav.getByRole('link', { name: 'Dashboard' }).getAttribute('href')).toBe('/dashboard');
    expect(nav.getByRole('link', { name: 'Knowledge' }).getAttribute('href')).toBe('/knowledge?session_id=A');
    expect(view.container.querySelector('.sidebar-rail')).toBeNull();
    await fireEvent.click(nav.getByRole('button', { name: /Collapse sidebar/ }));
    expect(get(layout).leftCollapsed).toBe(true);
    expect(view.container.querySelector('aside')?.style.width).toBe('0px');
    expect(view.container.querySelector('.sidebar-drawer')?.getAttribute('aria-hidden')).toBe('true');
    expect((view.container.querySelector('.sidebar-drawer') as HTMLElement).inert).toBe(true);
  });

  it.each([
    { path: '/', label: 'Sessions' },
    { path: '/dashboard', label: 'Dashboard' },
    { path: '/knowledge', label: 'Knowledge' },
  ])('keeps the drawer open when activating the current $label page', async ({ path, label }) => {
    (page as unknown as Writable<unknown>).set({ url: new URL(`http://localhost${path}`) });
    const view = render(SessionSidebar);
    const link = view.getByRole('link', { name: label });
    const event = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 });
    await fireEvent(link, event);
    expect(event.defaultPrevented).toBe(true);
    expect(get(layout).leftCollapsed).toBe(false);
    expect(view.container.querySelector('.sidebar-drawer')?.getAttribute('aria-hidden')).toBe('false');
    expect(view.getByRole('link', { name: label })).toBe(link);
  });

  it('uses the shared close dialog, dismisses without closing, and confirms the intended session', async () => {
    load([session('A', 'D:/one')]);
    const view = render(SessionSidebar);
    await fireEvent.click(view.getByRole('button', { name: 'Close Session' }));
    const body = within(document.body);
    const dialog = await body.findByRole('dialog', { name: 'Close Session?' });
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    expect(view.container.contains(dialog)).toBe(false);
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(body.queryByRole('dialog')).toBeNull());
    expect(launchMocks.closeSession).not.toHaveBeenCalled();
    await fireEvent.click(view.getByRole('button', { name: 'Close Session' }));
    const escaped = await body.findByRole('dialog', { name: 'Close Session?' });
    await fireEvent.keyDown(escaped, { key: 'Escape' });
    await waitFor(() => expect(body.queryByRole('dialog')).toBeNull());
    expect(launchMocks.closeSession).not.toHaveBeenCalled();
    await fireEvent.click(view.getByRole('button', { name: 'Close Session' }));
    const reopened = await body.findByRole('dialog', { name: 'Close Session?' });
    await fireEvent.click(within(reopened).getByRole('button', { name: 'Close Session' }));
    await waitFor(() => expect(launchMocks.closeSession).toHaveBeenCalledTimes(1));
    expect(launchMocks.closeSession).toHaveBeenCalledWith('A');
    await waitFor(() => expect(body.queryByRole('dialog')).toBeNull());
  });

  it('keeps the close confirmation open on Escape while closing is in progress', async () => {
    let finish!: () => void;
    launchMocks.closeSession.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    load([session('A', 'D:/one')]);
    const view = render(SessionSidebar);
    await fireEvent.click(view.getByRole('button', { name: 'Close Session' }));
    const body = within(document.body);
    const dialog = await body.findByRole('dialog', { name: 'Close Session?' });
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Close Session' }));
    expect((within(dialog).getByRole('button', { name: 'Cancel' }) as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(body.getByRole('dialog', { name: 'Close Session?' })).toBe(dialog);
    expect(launchMocks.closeSession).toHaveBeenCalledTimes(1);
    finish();
    await waitFor(() => expect(body.queryByRole('dialog')).toBeNull());
  });

  it('dismisses an idle close confirmation through its backdrop without closing the session', async () => {
    load([session('A', 'D:/one')]);
    const view = render(SessionSidebar);
    await fireEvent.click(view.getByRole('button', { name: 'Close Session' }));
    const body = within(document.body);
    await body.findByRole('dialog', { name: 'Close Session?' });
    await clickCloseBackdrop();
    await waitFor(() => expect(body.queryByRole('dialog')).toBeNull());
    expect(launchMocks.closeSession).not.toHaveBeenCalled();
  });

  it('keeps the close confirmation open on backdrop interaction while closing is in progress', async () => {
    let finish!: () => void;
    launchMocks.closeSession.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    load([session('A', 'D:/one')]);
    const view = render(SessionSidebar);
    await fireEvent.click(view.getByRole('button', { name: 'Close Session' }));
    const body = within(document.body);
    const dialog = await body.findByRole('dialog', { name: 'Close Session?' });
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Close Session' }));
    expect((within(dialog).getByRole('button', { name: 'Cancel' }) as HTMLButtonElement).disabled).toBe(true);
    await clickCloseBackdrop();
    expect(body.getByRole('dialog', { name: 'Close Session?' })).toBe(dialog);
    expect(launchMocks.closeSession).toHaveBeenCalledTimes(1);
    finish();
    await waitFor(() => expect(body.queryByRole('dialog')).toBeNull());
  });
});
