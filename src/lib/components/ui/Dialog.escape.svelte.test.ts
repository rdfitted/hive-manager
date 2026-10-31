import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, waitFor } from '@testing-library/svelte';
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
  sessionViewMode: 'focus' | 'grid';
}

const testMocks = vi.hoisted(() => ({
  setLayoutState: undefined as ((state: TestLayoutState) => void) | undefined,
  toggleLeft: vi.fn(),
  toggleRight: vi.fn(),
  setMaximizedTerminalId: vi.fn(),
  toggleMaximizedTerminal: vi.fn(),
  setSessionViewMode: vi.fn(),
  toggleSessionViewMode: vi.fn(),
  setSessionId: vi.fn(),
  loadSessions: vi.fn().mockResolvedValue(undefined),
  fetchCliHealth: vi.fn().mockResolvedValue({}),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
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
    sessionViewMode: 'grid',
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
  testMocks.setSessionViewMode.mockImplementation((mode: 'focus' | 'grid') => {
    state.update((current) => ({ ...current, sessionViewMode: mode }));
  });
  testMocks.toggleSessionViewMode.mockImplementation(() => {
    state.update((current) => ({
      ...current,
      sessionViewMode: current.sessionViewMode === 'focus' ? 'grid' : 'focus',
    }));
  });

  return {
    layout: {
      subscribe: state.subscribe,
      toggleLeft: testMocks.toggleLeft,
      toggleRight: testMocks.toggleRight,
      setMaximizedTerminalId: testMocks.setMaximizedTerminalId,
      toggleMaximizedTerminal: testMocks.toggleMaximizedTerminal,
      setSessionViewMode: testMocks.setSessionViewMode,
      toggleSessionViewMode: testMocks.toggleSessionViewMode,
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

vi.mock('$lib/components/LaunchDialog.svelte', () => ({ default: () => {} }));
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
import Page from '../../../routes/+page.svelte';
import Dialog from './Dialog.svelte';
import { layout } from '$lib/stores/layout';
import { launch } from '$lib/stores/launch';
import TerminalGrid from '$lib/components/TerminalGrid.svelte';

const expandedLayout: TestLayoutState = {
  leftCollapsed: false, leftWidth: 250, rightCollapsed: false, rightWidth: 320,
  rightTab: 'status', sessionsCollapsed: false, recentCollapsed: true,
  agentsCollapsed: false, maximizedTerminalId: 'agent-1', sessionViewMode: 'grid',
};

beforeEach(() => {
  vi.clearAllMocks();
  testMocks.setLayoutState?.(expandedLayout);
  launch.close();
});
afterEach(() => { cleanup(); document.body.innerHTML = ''; });

async function escape() {
  const event = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
  (document.activeElement ?? document.body).dispatchEvent(event);
  await tick();
  await Promise.resolve();
  return event;
}
async function openDialog() {
  const onOpenChange = vi.fn();
  const view = render(Dialog, { open: false, label: 'Test dialog', onOpenChange });
  await view.rerender({ open: true });
  await waitFor(() => expect(document.querySelector('[role="dialog"]')).toBeTruthy());
  // Let Bits register its document Escape layer and capture the opener's focus.
  await new Promise((resolve) => requestAnimationFrame(resolve));
  return { view, onOpenChange };
}

it('G2: closes only the dialog, preserving panels and the maximized grid pane', async () => {
  render(Page);
  render(TerminalGrid, { props: {
    session: {
      id: 'session-1', session_type: { Hive: { worker_count: 1 } }, project_path: 'D:/project',
      state: 'Running', created_at: '2026-09-14T00:00:00Z',
      agents: [{ id: 'agent-1', role: { Worker: { index: 1, parent: null } }, status: 'Running', config: { cli: 'codex', flags: [] }, parent_id: null }],
    }, focusedAgentId: 'agent-1', onSelect: vi.fn(),
  }});
  const { onOpenChange } = await openDialog();
  const event = await escape();
  expect(get(layout).maximizedTerminalId, 'Escape must preserve the maximized grid pane behind a dialog').toBe('agent-1');
  expect(testMocks.toggleLeft).not.toHaveBeenCalled();
  expect(testMocks.toggleRight).not.toHaveBeenCalled();
  expect(event.defaultPrevented).toBe(true);
  await waitFor(() => expect(document.querySelector('[role="dialog"]')).toBeNull());
  expect(onOpenChange).toHaveBeenCalledWith(false);
});

it('G2: landing capture leaves the landing open while Escape closes the dialog', async () => {
  launch.open();
  render(Page);
  const { onOpenChange } = await openDialog();
  await escape();
  expect(get(launch).landingOpen, 'Escape must preserve the landing behind a dialog').toBe(true);
  await waitFor(() => expect(document.querySelector('[role="dialog"]')).toBeNull());
  expect(onOpenChange).toHaveBeenCalledWith(false);
  expect(testMocks.toggleLeft).not.toHaveBeenCalled();
  expect(testMocks.toggleRight).not.toHaveBeenCalled();
});

it.each(['button', 'xterm textarea'])('G2: restores focus to the %s opener', async (kind) => {
  const opener = document.createElement(kind === 'button' ? 'button' : 'textarea');
  const host = document.createElement('div');
  if (kind !== 'button') host.className = 'xterm';
  host.append(opener); document.body.append(host); opener.focus();
  const { onOpenChange } = await openDialog();
  await escape();
  await waitFor(() => expect(document.querySelector('[role="dialog"]')).toBeNull());
  expect(onOpenChange).toHaveBeenCalledWith(false);
  await waitFor(() => expect(document.activeElement).toBe(opener));
});

it('exposes alert semantics and labelledby on the shared surface', async () => {
  const title = document.createElement('h2'); title.id = 'alert-title'; title.textContent = 'Confirm'; document.body.append(title);
  render(Dialog, { open: true, variant: 'alert', labelledby: title.id, class: 'custom-dialog' });
  const dialog = await waitFor(() => {
    const node = document.querySelector('[role="alertdialog"]'); expect(node).toBeTruthy(); return node!;
  });
  expect(dialog.getAttribute('aria-modal')).toBe('true');
  expect(dialog.getAttribute('aria-labelledby')).toBe(title.id);
  expect(dialog.classList.contains('custom-dialog')).toBe(true);
});

it('D5: keeps chrome interactions live without dismissing, while other outside interactions dismiss', async () => {
  const chrome = document.createElement('div'); chrome.dataset.windowChrome = '';
  const control = document.createElement('button');
  const icon = document.createElement('span'); control.append(icon); chrome.append(control); document.body.append(chrome);
  const ordinary = document.createElement('button'); document.body.append(ordinary);
  const { onOpenChange } = await openDialog();
  const pointer = () => new MouseEvent('pointerdown', { bubbles: true, cancelable: true, button: 0, clientX: 100, clientY: 100 });
  const click = vi.fn(); control.addEventListener('click', click);
  icon.dispatchEvent(pointer()); icon.click();
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect(click).toHaveBeenCalledOnce();
  expect(document.querySelector('[role="dialog"]')).toBeTruthy();
  expect(onOpenChange).not.toHaveBeenCalled();
  ordinary.dispatchEvent(pointer());
  await waitFor(() => expect(document.querySelector('[role="dialog"]')).toBeNull());
  expect(onOpenChange).toHaveBeenCalledWith(false);
});
