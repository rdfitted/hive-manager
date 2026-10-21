import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get, type Writable } from 'svelte/store';
import type { AgentInfo } from '$lib/stores/sessions';

interface TestLayoutState {
  leftCollapsed: boolean;
  leftWidth: number;
  rightCollapsed: boolean;
  rightWidth: number;
  rightTab: 'status';
  sessionsCollapsed: boolean;
  recentCollapsed: boolean;
  agentsCollapsed: boolean;
  maximizedTerminalId: string | null;
}

const testMocks = vi.hoisted(() => ({
  setLayoutState: undefined as ((state: TestLayoutState) => void) | undefined,
  toggleLeft: vi.fn(),
  toggleRight: vi.fn(),
  setMaximizedTerminalId: vi.fn(),
  toggleMaximizedTerminal: vi.fn(),
  setSessionId: vi.fn(),
  loadSessions: vi.fn().mockResolvedValue(undefined),
  fetchCliHealth: vi.fn().mockResolvedValue({}),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('phosphor-svelte', () => ({
  ArrowsIn: () => {},
  ArrowsOut: () => {},
  ArrowsSplit: () => {},
  Check: () => {},
  Circle: () => {},
  ClockCounterClockwise: () => {},
  Crown: () => {},
  Hourglass: () => {},
  Plus: () => {},
  Scales: () => {},
  X: () => {},
}));

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  const activeSession = writable({
    id: 'session-a',
    state: 'Running',
    session_type: { Hive: {} },
    project_path: 'D:/project',
    worktree_path: 'D:/project',
    default_cli: 'codex',
    default_model: 'gpt-6.1-sol',
    default_principal_cli: 'codex',
    default_principal_model: 'gpt-6.1-sol',
    default_principal_flags: [],
  });
  const activeAgents = writable<AgentInfo[]>([]);

  return {
    sessions: {
      loadSessions: testMocks.loadSessions,
      launchHiveV2: vi.fn(),
      launchFusion: vi.fn(),
      launchDebate: vi.fn(),
    },
    activeSession,
    activeAgents,
    serdeEnumVariantName: (value: unknown) => {
      if (typeof value === 'string') return value;
      if (value && typeof value === 'object') return Object.keys(value)[0];
      return undefined;
    },
  };
});

vi.mock('$lib/stores/layout', async () => {
  const { writable } = await import('svelte/store');
  const state = writable<TestLayoutState>({
    leftCollapsed: false,
    leftWidth: 250,
    rightCollapsed: false,
    rightWidth: 320,
    rightTab: 'status',
    sessionsCollapsed: false,
    recentCollapsed: true,
    agentsCollapsed: false,
    maximizedTerminalId: null,
  });
  testMocks.setLayoutState = state.set;
  testMocks.toggleLeft.mockImplementation(() => {
    state.update((current) => ({ ...current, leftCollapsed: !current.leftCollapsed }));
  });
  testMocks.toggleRight.mockImplementation(() => {
    state.update((current) => ({ ...current, rightCollapsed: !current.rightCollapsed }));
  });
  testMocks.setMaximizedTerminalId.mockImplementation((id: string | null) => {
    state.update((current) => ({ ...current, maximizedTerminalId: id }));
  });
  testMocks.toggleMaximizedTerminal.mockImplementation((id: string) => {
    state.update((current) => ({
      ...current,
      maximizedTerminalId: current.maximizedTerminalId === id ? null : id,
    }));
  });

  return {
    layout: {
      subscribe: state.subscribe,
      toggleLeft: testMocks.toggleLeft,
      toggleRight: testMocks.toggleRight,
      setMaximizedTerminalId: testMocks.setMaximizedTerminalId,
      toggleMaximizedTerminal: testMocks.toggleMaximizedTerminal,
    },
  };
});

vi.mock('$lib/stores/coordination', () => ({
  coordination: {
    setSessionId: testMocks.setSessionId,
    addWorker: vi.fn(),
  },
}));

vi.mock('$lib/stores/ui', async () => {
  const { writable } = await import('svelte/store');
  const state = writable({ focusedAgentId: null as string | null });
  return {
    ui: {
      subscribe: state.subscribe,
      setFocusedAgent: vi.fn((id: string | null) => {
        state.update((current) => ({ ...current, focusedAgentId: id }));
      }),
      setSelectedAgent: vi.fn(),
    },
  };
});

vi.mock('$lib/stores/pendingContext', () => ({
  pendingContext: { capture: vi.fn() },
}));

vi.mock('$lib/stores/shell', () => ({
  shell: {
    openAddWorker: vi.fn(),
    requestStartAction: vi.fn(),
  },
}));

vi.mock('$lib/stores/scratchTerminals', async () => {
  const { writable } = await import('svelte/store');
  const state = writable({ panesBySession: {}, focusedBySession: {} });
  return {
    scratchTerminals: {
      subscribe: state.subscribe,
      add: vi.fn(),
      remove: vi.fn(),
      focus: vi.fn(),
      clearSession: vi.fn(),
    },
    shellCommand: vi.fn(() => ({ command: 'powershell', args: [] })),
  };
});

vi.mock('$lib/components/SessionSidebar.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/RightPanel.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/ShortcutsOverlay.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/UpdateChecker.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/FusionPanel.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/DebatePanel.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/session/SessionOverview.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/AgentConfigEditor.svelte', () => ({
  default: () => {},
  fetchCliHealth: testMocks.fetchCliHealth,
}));
vi.mock('$lib/components/composer/Composer.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/Terminal.svelte', () => ({
  default: () => {},
  readTerminalSelection: vi.fn(() => null),
}));

import { activeAgents } from '$lib/stores/sessions';
import { ui } from '$lib/stores/ui';
import Page from './+page.svelte';


// Isolated welcome coverage; page.svelte.test.ts remains owned by lane AS.
beforeEach(async () => {
  vi.clearAllMocks();
  const { activeSession, activeAgents } = await import('$lib/stores/sessions');
  (activeSession as unknown as Writable<unknown>).set(null);
  (activeAgents as Writable<AgentInfo[]>).set([]);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('welcome launch entry', () => {
  it('opens the default Hive composer through the existing shell action', async () => {
    const { shell } = await import('$lib/stores/shell');
    const view = render(Page);
    await fireEvent.click(view.getByRole('button', { name: 'Start a session' }));
    expect(shell.requestStartAction).toHaveBeenCalledTimes(1);
    expect(shell.requestStartAction).toHaveBeenCalledWith('hive');
  });
  it.each(['Hive', 'Fusion', 'Debate', 'Open recent'])('%s retains its existing entry action', async (label) => {
    const { shell } = await import('$lib/stores/shell');
    const view = render(Page);
    await fireEvent.click(view.getByRole('button', { name: new RegExp(label) }));
    expect(shell.requestStartAction).toHaveBeenCalledTimes(1);
    expect(shell.requestStartAction).toHaveBeenCalledWith(label === 'Open recent' ? 'recent' : label.toLowerCase());
  });
  it('keeps the platform shortcut hint visible', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel');
    const view = render(Page);
    expect(view.getByText('⌘/')).toBeTruthy();
  });
});
