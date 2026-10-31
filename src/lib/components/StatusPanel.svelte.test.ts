import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import { sessions, type Session } from '$lib/stores/sessions';
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

describe('StatusPanel shared confirmations', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  const actions = [
    { button: 'Close Session', title: 'Close Session?', endpoint: null, rationale: null },
    { button: 'Force Fail', title: 'Force Fail Milestone?', endpoint: '/qa/force-fail', rationale: 'Force fail from the Status panel' },
    { button: 'Force Pass', title: 'Force Pass Milestone?', endpoint: '/qa/force-pass', rationale: 'Force pass from the Status panel' },
    { button: 'Skip QA', title: 'Skip QA?', endpoint: '/qa/force-pass', rationale: 'Skip QA from the Status panel' },
  ];

  function openPanel() {
    tauriMocks.invoke.mockResolvedValue(null);
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => new Response('{}', { status: 200 }));
    vi.stubGlobal('fetch', fetchMock);
    const closeSession = vi.spyOn(sessions, 'closeSession').mockResolvedValue(undefined);
    const view = render(StatusPanel);
    const session = fakeSession('Hive', true);
    session.state = 'QaInProgress';
    storeMocks.setActiveSession?.(session);
    return { ...view, fetchMock, closeSession };
  }

  it.each(actions)('keeps $button guarded through Cancel and Escape, then submits the same action', async (action) => {
    const view = openPanel();
    const body = within(document.body);
    const trigger = await body.findByRole('button', { name: action.button });
    await fireEvent.click(trigger);
    const firstDialog = body.getByRole('dialog', { name: action.title });
    expect(firstDialog.getAttribute('aria-modal')).toBe('true');
    expect(view.container.contains(firstDialog)).toBe(false);
    await fireEvent.click(within(firstDialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(body.queryByRole('dialog')).toBeNull());
    expect(view.closeSession).not.toHaveBeenCalled();
    expect(view.fetchMock.mock.calls.filter((call) => call[1]?.method === 'POST')).toHaveLength(0);

    await fireEvent.click(trigger);
    const escaped = body.getByRole('dialog', { name: action.title });
    await fireEvent.keyDown(escaped, { key: 'Escape', code: 'Escape' });
    await waitFor(() => expect(body.queryByRole('dialog')).toBeNull());
    expect(view.closeSession).not.toHaveBeenCalled();
    expect(view.fetchMock.mock.calls.filter((call) => call[1]?.method === 'POST')).toHaveLength(0);

    await fireEvent.click(trigger);
    const confirmed = body.getByRole('dialog', { name: action.title });
    await fireEvent.click(within(confirmed).getByRole('button', { name: action.button }));
    if (action.endpoint) {
      await waitFor(() => expect(view.fetchMock).toHaveBeenCalledWith(
        expect.stringContaining(`/api/sessions/qa-hive${action.endpoint}`),
        expect.objectContaining({ method: 'POST', body: JSON.stringify({ confirm: true, rationale: action.rationale }) }),
      ));
      expect(view.closeSession).not.toHaveBeenCalled();
    } else {
      await waitFor(() => expect(view.closeSession).toHaveBeenCalledTimes(1));
      expect(view.closeSession).toHaveBeenCalledWith('qa-hive');
    }
    await waitFor(() => expect(body.queryByRole('dialog')).toBeNull());
  });

  it.each(['Close Session', 'Force Fail'])('keeps %s confirmation open while its request is pending', async (button) => {
    const view = openPanel();
    const closed = deferred<void>();
    const failed = deferred<Response>();
    view.closeSession.mockReturnValue(closed.promise);
    view.fetchMock.mockImplementation(async (_input, init) => init?.method === 'POST' ? failed.promise : new Response('{}', { status: 200 }));
    const body = within(document.body);
    await fireEvent.click(await body.findByRole('button', { name: button }));
    const dialog = body.getByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: button }));
    expect((within(dialog).getByRole('button', { name: 'Cancel' }) as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.keyDown(dialog, { key: 'Escape', code: 'Escape' });
    expect(body.getByRole('dialog')).toBe(dialog);
    closed.resolve();
    failed.resolve(new Response('{}', { status: 200 }));
    await waitFor(() => expect(body.queryByRole('dialog')).toBeNull());
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
