import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import { tick } from 'svelte';
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
    default_model: 'gpt-5.6-sol',
    default_principal_cli: 'codex',
    default_principal_model: 'gpt-5.6-sol',
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

import Page from './+page.svelte';
import TerminalGrid from '$lib/components/TerminalGrid.svelte';

const expandedLayout: TestLayoutState = {
  leftCollapsed: false,
  leftWidth: 250,
  rightCollapsed: false,
  rightWidth: 320,
  rightTab: 'status',
  sessionsCollapsed: false,
  recentCollapsed: true,
  agentsCollapsed: false,
  maximizedTerminalId: null,
};

function setLayoutState(state: TestLayoutState): void {
  if (!testMocks.setLayoutState) throw new Error('Layout mock was not initialized');
  testMocks.setLayoutState(state);
}

async function pressEscape(): Promise<KeyboardEvent> {
  const event = new KeyboardEvent('keydown', {
    key: 'Escape',
    bubbles: true,
    cancelable: true,
  });
  window.dispatchEvent(event);
  await Promise.resolve();
  await tick();
  return event;
}

beforeEach(() => {
  vi.clearAllMocks();
  setLayoutState(expandedLayout);
});

afterEach(() => {
  cleanup();
  document.body.innerHTML = '';
});

describe('page Escape priority', () => {
  it('closes Add Worker without collapsing either expanded panel', async () => {
    const view = render(Page);
    await fireEvent.click(view.getByRole('button', { name: 'Add Principal' }));
    expect(view.getByRole('dialog')).toBeTruthy();

    const event = await pressEscape();

    expect(event.defaultPrevented).toBe(true);
    expect(view.queryByRole('dialog')).toBeNull();
    expect(testMocks.toggleLeft).not.toHaveBeenCalled();
    expect(testMocks.toggleRight).not.toHaveBeenCalled();
  });

  it('collapses expanded panels when Add Worker is closed and no modal is open', async () => {
    render(Page);

    const event = await pressEscape();

    expect(event.defaultPrevented).toBe(true);
    expect(testMocks.toggleLeft).toHaveBeenCalledTimes(1);
    expect(testMocks.toggleRight).toHaveBeenCalledTimes(1);
  });

  it('leaves panels and a maximized terminal unchanged behind an open modal', async () => {
    setLayoutState({ ...expandedLayout, maximizedTerminalId: 'agent-1' });
    const page = render(Page);
    render(TerminalGrid, {
      props: {
        agents: [{
          id: 'agent-1',
          role: { Worker: { index: 1, parent: null } },
          status: 'Running',
          config: { cli: 'codex', flags: [] },
          parent_id: null,
        }],
        focusedAgentId: 'agent-1',
        onSelect: vi.fn(),
      },
    });
    await tick();

    const modal = document.createElement('div');
    modal.setAttribute('role', 'dialog');
    modal.setAttribute('aria-modal', 'true');
    document.body.append(modal);
    const opener = page.getByRole('button', { name: 'Add Principal' });
    opener.focus();
    testMocks.toggleLeft.mockClear();
    testMocks.toggleRight.mockClear();
    testMocks.setMaximizedTerminalId.mockClear();

    const event = await pressEscape();

    expect(event.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(opener);
    expect(testMocks.toggleLeft).not.toHaveBeenCalled();
    expect(testMocks.toggleRight).not.toHaveBeenCalled();
    expect(testMocks.setMaximizedTerminalId).not.toHaveBeenCalled();
  });
});
