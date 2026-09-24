import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import type { Session } from '$lib/stores/sessions';
import StatusPanel from './StatusPanel.svelte';

const tauriMocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  isTauri: vi.fn(() => true),
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: tauriMocks.invoke,
  isTauri: tauriMocks.isTauri,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: tauriMocks.listen,
}));

const storeMocks = vi.hoisted(() => ({
  setActiveSession: undefined as ((session: Session | null) => void) | undefined,
}));

vi.mock('$lib/stores/sessions', async () => {
  const actual = await vi.importActual<typeof import('$lib/stores/sessions')>('$lib/stores/sessions');
  const { derived, writable } = await import('svelte/store');
  const activeSession = writable<Session | null>(null);
  storeMocks.setActiveSession = activeSession.set;
  return {
    ...actual,
    activeSession,
    activeAgents: derived(activeSession, (session) => session?.agents ?? []),
  };
});

function fakeSession(mode: 'Hive' | 'Solo' | 'Fusion', withEvaluator: boolean): Session {
  return {
    id: `qa-${mode.toLowerCase()}`,
    session_type: mode === 'Hive'
      ? { Hive: { worker_count: 1 } }
      : mode === 'Solo'
        ? { Solo: { cli: 'claude' } }
        : { Fusion: { variants: [] } },
    project_path: 'C:/code/project',
    state: 'Running',
    created_at: '2026-09-24T12:00:00Z',
    agents: withEvaluator
      ? [{ id: 'evaluator', role: 'Evaluator', status: 'Running', config: { cli: 'claude', flags: [] }, parent_id: null }]
      : [],
  } as Session;
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function cliItem(container: HTMLElement, label: string): HTMLElement {
  const item = Array.from(container.querySelectorAll<HTMLElement>('.cli-health-item'))
    .find((candidate) => candidate.querySelector('.cli-health-name')?.textContent?.trim() === label);
  if (!item) throw new Error(`Missing CLI health item for ${label}`);
  return item;
}

function expectCliState(
  item: HTMLElement,
  label: string,
  message: string,
  tone: 'healthy' | 'warning' | 'error' | 'pending',
) {
  const badge = item.querySelector<HTMLElement>('.cli-health-badge');
  const detail = item.querySelector<HTMLElement>('.cli-health-detail');
  expect(badge?.textContent?.trim()).toBe(label);
  expect(detail?.textContent?.trim()).toBe(message);
  expect(badge?.classList.contains(tone)).toBe(true);
  expect(detail?.classList.contains(tone)).toBe(true);
}

afterEach(() => {
  cleanup();
  storeMocks.setActiveSession?.(null);
  tauriMocks.invoke.mockReset();
  tauriMocks.isTauri.mockReset();
  tauriMocks.isTauri.mockReturnValue(true);
});

describe('StatusPanel QA status', () => {
  it.each(['Hive', 'Solo'] as const)('shows QA Off for a %s session without an Evaluator', async (mode) => {
    tauriMocks.invoke.mockResolvedValue(null);
    const view = render(StatusPanel);
    storeMocks.setActiveSession?.(fakeSession(mode, false));
    await waitFor(() => expect(view.getByText('QA Off')).toBeTruthy());
    expect(view.getByText('QA Off').classList.contains('status-warning')).toBe(true);
  });

  it('does not show QA Off when an Evaluator is present or for Fusion', async () => {
    tauriMocks.invoke.mockResolvedValue(null);
    const view = render(StatusPanel);
    storeMocks.setActiveSession?.(fakeSession('Hive', true));
    await waitFor(() => expect(view.getByText('Session Info')).toBeTruthy());
    expect(view.queryByText('QA Off')).toBeNull();
    storeMocks.setActiveSession?.(fakeSession('Fusion', false));
    await waitFor(() => expect(view.queryByText('QA Off')).toBeNull());
  });
});

describe('StatusPanel CLI health', () => {
  it('renders canonical states and exposes a toggled disclosure relationship', async () => {
    const request = deferred<unknown>();
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === 'get_cli_health') return request.promise;
      return Promise.resolve(null);
    });

    const { container, getByRole } = render(StatusPanel);
    const disclosure = getByRole('button', { name: 'CLI Health' });

    expect(disclosure.getAttribute('aria-expanded')).toBe('true');
    expect(disclosure.getAttribute('aria-controls')).toBe('cli-health-list');
    expect(container.querySelector('#cli-health-list')).not.toBeNull();

    await waitFor(() => {
      expectCliState(
        cliItem(container, 'Claude Code'),
        'Checking…',
        'Checking whether this CLI can launch on this machine.',
        'pending',
      );
    });

    request.resolve({
      clis: [
        {
          cli: 'claude',
          resolved: false,
          binPath: null,
          loggedIn: 'unknown',
          detail: '',
          staleHint: true,
        },
        {
          cli: 'cursor',
          resolved: true,
          binPath: '/usr/bin/wsl',
          loggedIn: 'no',
          detail: '',
          staleHint: false,
        },
        {
          cli: 'droid',
          resolved: true,
          binPath: '/usr/bin/droid',
          loggedIn: 'unknown',
          detail: '',
          staleHint: false,
        },
        {
          cli: 'opencode',
          resolved: true,
          binPath: '/usr/bin/opencode',
          loggedIn: 'yes',
          detail: '',
          staleHint: false,
        },
      ],
    });

    await waitFor(() => {
      expectCliState(
        cliItem(container, 'Claude Code'),
        'Not on current PATH',
        'The executable is missing from the current PATH. Restarting Hive Manager after updating PATH may help.',
        'warning',
      );
      expectCliState(
        cliItem(container, 'Cursor'),
        'Login required',
        'The CLI is installed but needs authentication.',
        'error',
      );
      expectCliState(
        cliItem(container, 'Droid'),
        'Auth unknown',
        'The CLI is installed, but authentication cannot be verified automatically.',
        'warning',
      );
      expectCliState(
        cliItem(container, 'OpenCode'),
        'Ready',
        'The CLI is installed and authenticated.',
        'healthy',
      );
      expectCliState(
        cliItem(container, 'Codex'),
        'Not checked',
        'CLI health has not been checked yet.',
        'pending',
      );
    });

    await fireEvent.click(disclosure);
    expect(disclosure.getAttribute('aria-expanded')).toBe('false');
    expect(disclosure.getAttribute('aria-controls')).toBe('cli-health-list');
    expect(container.querySelector('#cli-health-list')).toBeNull();

    await fireEvent.click(disclosure);
    expect(disclosure.getAttribute('aria-expanded')).toBe('true');
    expect(container.querySelector('#cli-health-list')).not.toBeNull();

    tauriMocks.invoke.mockRejectedValueOnce(new Error('health backend unavailable'));
    await fireEvent.click(getByRole('button', { name: 'Refresh' }));

    await waitFor(() => {
      expectCliState(
        cliItem(container, 'Codex'),
        'Health unavailable',
        'health backend unavailable',
        'warning',
      );
    });
  });
});
