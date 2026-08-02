import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { get } from 'svelte/store';
import { tick } from 'svelte';

const testMocks = vi.hoisted(() => ({
  toggleLeft: vi.fn(),
  launchHiveV2: vi.fn().mockResolvedValue(undefined),
  launchFusion: vi.fn().mockResolvedValue(undefined),
  launchDebate: vi.fn().mockResolvedValue(undefined),
  fetchCliHealth: vi.fn().mockResolvedValue({}),
  sessionSidebar: vi.fn(),
}));

vi.mock('$lib/stores/layout', () => ({
  layout: {
    toggleLeft: testMocks.toggleLeft,
  },
}));

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    sessions: {
      launchHiveV2: testMocks.launchHiveV2,
      launchFusion: testMocks.launchFusion,
      launchDebate: testMocks.launchDebate,
    },
    activeSession: writable({
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
    }),
    activeAgents: writable([]),
    serdeEnumVariantName: (value: unknown) => {
      if (typeof value === 'string') return value;
      if (value && typeof value === 'object') return Object.keys(value)[0];
      return undefined;
    },
  };
});

vi.mock('$lib/stores/coordination', () => ({
  coordination: {
    addWorker: vi.fn(),
  },
}));

vi.mock('$lib/components/SessionSidebar.svelte', () => ({
  default: (anchor: unknown, props: unknown) => testMocks.sessionSidebar(anchor, props),
}));
vi.mock('$lib/components/AgentConfigEditor.svelte', () => ({
  default: () => {},
  fetchCliHealth: testMocks.fetchCliHealth,
}));
vi.mock('$lib/components/composer/Composer.svelte', () => ({ default: () => {} }));

import Layout from './+layout.svelte';
import { shell } from '$lib/stores/shell';

interface SidebarProps {
  onOpenAddWorker: () => void;
  startAction: { id: number; action: string } | null;
}

function getSidebarProps(): SidebarProps {
  const call = testMocks.sessionSidebar.mock.calls.at(-1);
  if (!call) throw new Error('SessionSidebar mock was not rendered');
  return call[1] as SidebarProps;
}

beforeEach(() => {
  vi.clearAllMocks();
  shell.closeAddWorker();
});

afterEach(() => {
  cleanup();
  shell.closeAddWorker();
  document.body.innerHTML = '';
});

describe('persistent app layout', () => {
  it('owns the real Add Worker dialog open and close path', async () => {
    const view = render(Layout);

    getSidebarProps().onOpenAddWorker();
    await tick();
    expect(view.getByRole('dialog')).toBeTruthy();

    await fireEvent.click(view.getByRole('button', { name: 'Cancel' }));

    await waitFor(() => expect(view.queryByRole('dialog')).toBeNull());
    expect(get(shell).addWorkerOpen).toBe(false);
  });

  it('passes repeat start actions to the persistent sidebar with increasing ids', async () => {
    render(Layout);
    const sidebar = getSidebarProps();

    shell.requestStartAction('recent');
    await tick();
    const first = sidebar.startAction;
    shell.requestStartAction('recent');
    await tick();
    const second = sidebar.startAction;

    expect(first).toMatchObject({ action: 'recent' });
    expect(second).toMatchObject({ action: 'recent' });
    expect(second?.id).toBeGreaterThan(first?.id ?? Number.NEGATIVE_INFINITY);
  });

  it('toggles the left sidebar exactly once for Ctrl+B', async () => {
    render(Layout);
    const event = new KeyboardEvent('keydown', {
      key: 'b',
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });

    window.dispatchEvent(event);
    await tick();

    expect(event.defaultPrevented).toBe(true);
    expect(testMocks.toggleLeft).toHaveBeenCalledTimes(1);
  });
});
