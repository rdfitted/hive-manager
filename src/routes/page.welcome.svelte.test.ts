// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get, type Writable } from 'svelte/store';
import type { Session } from '$lib/stores/sessions';

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(), goto: vi.fn().mockResolvedValue(undefined), open: vi.fn(),
  launchHiveV2: vi.fn(), fetchCliHealth: vi.fn().mockResolvedValue({}),
  terminalsCreated: 0, terminalsDestroyed: 0, gridsCreated: 0, gridsDestroyed: 0,
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: mocks.open }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('$app/navigation', () => ({ goto: mocks.goto }));
vi.mock('$app/stores', async () => ({ page: (await import('svelte/store')).readable({ url: new URL('http://localhost/') }) }));
vi.mock('$lib/stores/sessions', async () => {
  const { writable, get } = await import('svelte/store');
  const activeSession = writable<Session | null>(null);
  const activeAgents = writable([]);
  const state = writable({ sessions: [] as Session[] });
  return {
    sessions: {
      ...state, loadSessions: vi.fn().mockResolvedValue(undefined),
      setActiveSession: vi.fn((id: string | null) => activeSession.set(get(state).sessions.find((session) => session.id === id) ?? null)),
      launchHiveV2: mocks.launchHiveV2, launchResearch: vi.fn(),
      launchFusion: vi.fn(), launchSolo: vi.fn(), launchDebate: vi.fn(),
    },
    activeSession, activeAgents,
    serdeEnumVariantName: (value: unknown) => typeof value === 'string' ? value : value && typeof value === 'object' ? Object.keys(value)[0] : undefined,
  };
});
vi.mock('$lib/stores/coordination', () => ({ coordination: { setSessionId: vi.fn() } }));
vi.mock('$lib/stores/conversations', async () => ({ heartbeatStore: { ...(await import('svelte/store')).writable({ agents: {}, staleAgents: new Set(), stalledAgents: new Set() }), loadHeartbeats: vi.fn().mockResolvedValue(undefined) }, conversationStore: { pollMessages: vi.fn().mockResolvedValue(undefined) } }));
vi.mock('$lib/stores/cells', async () => ({ cells: { ...(await import('svelte/store')).writable({ cells: {}, sessionNotFound: false }), fetchCells: vi.fn().mockResolvedValue(undefined), setExternalRefreshHandler: vi.fn() } }));
vi.mock('$lib/stores/agents', () => ({ agents: { fetchAgents: vi.fn().mockResolvedValue(undefined) } }));
vi.mock('$lib/stores/events', async () => ({ events: { ...(await import('svelte/store')).writable({ events: [] }), connect: vi.fn(), disconnect: vi.fn() } }));
vi.mock('$lib/components/RightPanel.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/ShortcutsOverlay.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/UpdateChecker.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/FusionPanel.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/DebatePanel.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/session/SessionHeader.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/QueenControls.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/ResumeConfirmModal.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/AgentConfigEditor.svelte', () => ({ default: () => {}, fetchCliHealth: mocks.fetchCliHealth }));
vi.mock('$lib/components/composer/Composer.svelte', () => ({ default: () => {} }));
// Keep the real SessionOverview and TerminalGrid; instrument only the native renderer.
vi.mock('$lib/components/TerminalGrid.svelte', async () => {
  const actual = await vi.importActual<typeof import('$lib/components/TerminalGrid.svelte')>('$lib/components/TerminalGrid.svelte');
  const { onDestroy } = await import('svelte');
  return { default: (...args: Parameters<typeof actual.default>) => {
    mocks.gridsCreated++;
    onDestroy(() => { mocks.gridsDestroyed++; });
    return actual.default(...args);
  } };
});
vi.mock('$lib/components/Terminal.svelte', async () => {
  const { onDestroy } = await import('svelte');
  return { default: (anchor: Node) => {
    mocks.terminalsCreated++;
    const buffer = document.createElement('pre');
    buffer.dataset.terminalBuffer = 'true';
    buffer.textContent = 'existing scrollback';
    anchor.parentNode!.insertBefore(buffer, anchor);
    onDestroy(() => { mocks.terminalsDestroyed++; buffer.remove(); });
    return { focus: () => {} };
  }, readTerminalSelection: vi.fn(() => null) };
});

import Page from './+page.svelte';
import SessionSidebar from '$lib/components/SessionSidebar.svelte';
import { activeSession, activeAgents, sessions } from '$lib/stores/sessions';
import { launch } from '$lib/stores/launch';
import { shell } from '$lib/stores/shell';
import { selectedTemplate } from '$lib/stores/templates';
import { layout } from '$lib/stores/layout';

const project = 'D:/project';
const taskLabel = 'What would you like to work on?';
const active = activeSession as Writable<Session | null>;
const running = { id: 'running', name: 'Running hive', state: 'Running', session_type: { Hive: {} }, project_path: project, agents: [
  { id: 'worker', role: 'Queen', status: 'Running', config: { cli: 'codex', flags: [], label: 'Worker' }, parent_id: null },
] } as unknown as Session;

function activateSession() {
  (sessions as unknown as Writable<{ sessions: Session[] }>).set({ sessions: [running] });
  (activeAgents as Writable<Session['agents']>).set(running.agents);
  active.set(running);
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.terminalsCreated = mocks.terminalsDestroyed = mocks.gridsCreated = mocks.gridsDestroyed = 0;
  (sessions as unknown as Writable<{ sessions: Session[] }>).set({ sessions: [] });
  (activeAgents as Writable<Session['agents']>).set([]);
  active.set(null);
  launch.close();
  launch.open('hive');
  launch.close();
  launch.setDefaultProjectPath(project);
  if (get(layout).leftCollapsed) layout.toggleLeft();
  if (get(layout).sessionsCollapsed) layout.toggleSection('sessionsCollapsed');
  layout.setSessionViewMode('focus');
  layout.setMaximizedTerminalId(null);
  selectedTemplate.set(null);
  mocks.open.mockResolvedValue('D:/edited');
  mocks.invoke.mockImplementation(async (command: string) => command === 'get_current_directory' ? project : []);
  vi.stubGlobal('fetch', vi.fn(async (url: string) => new Response(JSON.stringify(
    url.includes('/api/templates') ? { templates: [], role_packs: [] }
      : url.includes('/api/preset-catalogue') ? { presets: [] }
      : url.includes('/api/tier-ladder') ? { cells: [], omissions: [] } : { clis: [] },
  ), { status: 200 })));
});
afterEach(() => { cleanup(); selectedTemplate.set(null); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

function textarea(view: { getByLabelText: (label: string) => HTMLElement }) {
  return view.getByLabelText(taskLabel) as HTMLTextAreaElement;
}

const componentSources = import.meta.glob<string>('../**/*.svelte', { query: '?raw', import: 'default', eager: true });

describe('inline composer landing', () => {
  it('G-landing-inline: renders the task textarea focused without modal ancestors', async () => {
    const view = render(Page);
    expect(textarea(view).closest('[role="dialog"], [aria-modal]')).toBeNull();
    expect(view.queryByRole('dialog')).toBeNull();
    expect(view.queryByRole('button', { name: 'Cancel' })).toBeNull();
    expect(view.queryByRole('heading', { name: 'Launch New Session' })).toBeNull();
    expect(view.getByText('Hive Manager')).toBeTruthy();
    await waitFor(() => expect(document.activeElement).toBe(textarea(view)));
  });

  it('Open recent retains its existing shell action', async () => {
    const request = vi.spyOn(shell, 'requestStartAction');
    const view = render(Page);
    await fireEvent.click(view.getByRole('button', { name: 'Open recent' }));
    expect(request).toHaveBeenCalledWith('recent');
  });

  it('keeps the platform shortcut hint visible', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel');
    const view = render(Page);
    expect(view.getByText('⌘/')).toBeTruthy();
  });

  it('removes the Start button and mode cards', () => {
    const view = render(Page);
    expect(view.queryByRole('button', { name: 'Start a session' })).toBeNull();
    expect(view.queryByRole('group', { name: 'Start a session' })).toBeNull();
    for (const mode of ['Hive', 'Fusion', 'Debate']) expect(view.queryByRole('button', { name: mode })).toBeNull();
  });

  it('Escape keeps the composer and its task text', async () => {
    const view = render(Page);
    const collapsed = get(layout).leftCollapsed;
    await fireEvent.input(textarea(view), { target: { value: 'Keep my task' } });
    await fireEvent.keyDown(window, { key: 'Escape' });
    await tick();
    expect(textarea(view).value).toBe('Keep my task');
    expect(get(layout).leftCollapsed).toBe(collapsed);
  });

  it('a launch error keeps the task text and the landing open', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    mocks.launchHiveV2.mockRejectedValueOnce(new Error('Launch unavailable'));
    const view = render(Page);
    await fireEvent.input(textarea(view), { target: { value: 'Keep after failure' } });
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    await waitFor(() => expect(view.getByRole('alert').textContent).toContain('Launch unavailable'));
    expect(textarea(view).value).toBe('Keep after failure');
    expect(get(activeSession)).toBeNull();
    error.mockRestore();
  });

  it('successful launch hides the landing and preserves the draft for its next request', async () => {
    mocks.launchHiveV2.mockImplementationOnce(async () => { activateSession(); return running; });
    const view = render(Page);
    await fireEvent.input(textarea(view), { target: { value: 'Draft through launch' } });
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    await waitFor(() => expect(view.queryByLabelText(taskLabel)).toBeNull());
    expect(get(activeSession)).toBe(running);
    launch.open();
    await tick();
    expect(textarea(view).value).toBe('Draft through launch');
  });

  it('G-single-state: Advanced and Back preserve the task and project edits', async () => {
    const view = render(Page);
    await fireEvent.input(textarea(view), { target: { value: 'Shared draft' } });
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
    await fireEvent.input(view.getByLabelText('Project Path'), { target: { value: 'D:/advanced-edit' } });
    await fireEvent.click(view.getByRole('button', { name: 'Back to composer' }));
    expect(textarea(view).value).toBe('Shared draft');
    expect(view.getByRole('button', { name: /Project D:\/advanced-edit/ })).toBeTruthy();
    await waitFor(() => expect(document.activeElement).toBe(textarea(view)));
  });

  it('repeated mode requests apply while the landing is already shown and refocus it', async () => {
    const view = render(Page);
    const recent = view.getByRole('button', { name: 'Open recent' });
    recent.focus();
    launch.open('fusion');
    await tick();
    expect((view.getByLabelText('Mode') as HTMLSelectElement).value).toBe('fusion');
    await waitFor(() => expect(document.activeElement).toBe(textarea(view)));
    await fireEvent.change(view.getByLabelText('Mode'), { target: { value: 'hive' } });
    launch.open('fusion');
    await tick();
    expect((view.getByLabelText('Mode') as HTMLSelectElement).value).toBe('fusion');
  });

  it('G-single-state: sidebar New Session and page start actions share the same instance', async () => {
    const view = render(Page);
    const sidebar = render(SessionSidebar, { props: { onLaunchHiveV2: mocks.launchHiveV2, startAction: null } });
    await fireEvent.click(sidebar.getByRole('button', { name: 'New Session' }));
    await fireEvent.input(textarea(view), { target: { value: 'Typed through sidebar entry' } });
    shell.requestStartAction('fusion');
    await sidebar.rerender({ onLaunchHiveV2: mocks.launchHiveV2, startAction: get(shell).startAction });
    expect(textarea(view).value).toBe('Typed through sidebar entry');
    expect((view.getByLabelText('Mode') as HTMLSelectElement).value).toBe('fusion');
    expect(view.getAllByLabelText(taskLabel)).toHaveLength(1);
  });

  it('G-no-remount-landing: New Session hides and restores the live grid without losing scrollback', async () => {
    activateSession();
    const view = render(Page);
    const sidebar = render(SessionSidebar, { props: { onLaunchHiveV2: mocks.launchHiveV2, startAction: null } });
    await tick();
    const buffer = view.container.querySelector<HTMLElement>('[data-terminal-buffer]')!;
    expect(buffer).toBeTruthy();
    buffer.textContent = 'line one\nline two';
    buffer.scrollTop = 42;
    const lifetimes = [mocks.terminalsCreated, mocks.terminalsDestroyed, mocks.gridsCreated, mocks.gridsDestroyed];
    await fireEvent.click(sidebar.getByRole('button', { name: 'New Session' }));
    expect(get(activeSession)).toBe(running);
    expect(sessions.setActiveSession).not.toHaveBeenCalled();
    expect(textarea(view).closest('[role="dialog"], [aria-modal]')).toBeNull();
    const sessionArea = view.container.querySelector<HTMLElement>('.terminal-area')!;
    expect(sessionArea.hidden, sessionArea.outerHTML.slice(0, 250)).toBe(true);
    expect(sessionArea.inert, sessionArea.outerHTML.slice(0, 250)).toBe(true);
    expect(buffer.isConnected).toBe(true);
    expect([mocks.terminalsCreated, mocks.terminalsDestroyed, mocks.gridsCreated, mocks.gridsDestroyed]).toEqual(lifetimes);
    await fireEvent.input(textarea(view), { target: { value: 'New draft while session runs' } });
    await fireEvent.click(sidebar.getByRole('button', { name: /Running hive/ }));
    expect(get(activeSession)).toBe(running);
    expect(view.queryByLabelText(taskLabel)).toBeNull();
    expect(view.container.querySelector<HTMLElement>('.terminal-area')!.hidden).toBe(false);
    expect(view.container.querySelector('[data-terminal-buffer]')).toBe(buffer);
    expect(buffer.textContent).toBe('line one\nline two');
    expect(buffer.scrollTop).toBe(42);
    expect([mocks.terminalsCreated, mocks.terminalsDestroyed, mocks.gridsCreated, mocks.gridsDestroyed]).toEqual(lifetimes);
    await fireEvent.click(sidebar.getByRole('button', { name: 'New Session' }));
    expect(textarea(view).value).toBe('New draft while session runs');
    expect(view.getAllByLabelText(taskLabel)).toHaveLength(1);
  });

  it('Escape with an active session closes the landing and keeps the same terminal grid', async () => {
    activateSession();
    layout.setSessionViewMode('grid');
    layout.setMaximizedTerminalId('worker');
    const view = render(Page);
    await tick();
    const buffer = view.container.querySelector('[data-terminal-buffer]');
    launch.open('hive');
    await tick();
    expect(textarea(view)).toBeTruthy();
    await fireEvent.keyDown(window, { key: 'Escape' });
    await tick();
    expect(view.queryByLabelText(taskLabel)).toBeNull();
    expect(view.container.querySelector('[data-terminal-buffer]')).toBe(buffer);
    expect(get(activeSession)).toBe(running);
    expect(mocks.terminalsDestroyed).toBe(0);
    expect(get(layout).maximizedTerminalId).toBe('worker');
  });

  it('G-single-state: exactly one LaunchDialog instance is mounted in Svelte source', () => {
    const mounts = Object.entries(componentSources).flatMap(([path, source]) =>
      [...source.matchAll(/<LaunchDialog(?:\s|>)/g)].map(() => path.replace('../', 'src/')),
    );
    expect(mounts).toHaveLength(1);
    expect(mounts[0]).toMatch(/(^|\/)\+page\.svelte$/);
  });

  it('uses the project default only while the project path is empty', async () => {
    const view = render(Page);
    expect(view.getByRole('button', { name: /Project D:\/project/ })).toBeTruthy();
    await fireEvent.click(view.getByRole('button', { name: /Project/ }));
    launch.setDefaultProjectPath('D:/later-default');
    await tick();
    expect(view.getByRole('button', { name: /Project D:\/edited/ })).toBeTruthy();
  });

  it('prefills the latest persisted project after loading, before falling back to cwd', async () => {
    launch.setDefaultProjectPath('');
    mocks.invoke.mockImplementation(async (command: string) => command === 'get_current_directory' ? 'D:/cwd' : [
      { id: 'latest', project_path: 'D:/most-recent', created_at: '2026-10-01', last_activity_at: '2026-10-22' },
    ]);
    const view = render(Page);
    render(SessionSidebar, { props: { onLaunchHiveV2: mocks.launchHiveV2, startAction: null } });
    await waitFor(() => expect(view.getByRole('button', { name: /Project D:\/most-recent/ })).toBeTruthy());
  });
});
