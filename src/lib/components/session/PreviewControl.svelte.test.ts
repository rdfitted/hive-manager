import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import type { Session } from '$lib/stores/sessions';
import PreviewControl from './PreviewControl.svelte';

/**
 * Regression cover for the dockable-preview header (issue #157).
 *
 * The load-bearing assertion here is that the URL input stays `type="text"`.
 * With `type="url"`, native constraint validation rejects scheme-less input
 * before `submit` ever fires, so `openPreview` never runs and the backend's
 * forgiving normalization is unreachable at runtime — while every Rust
 * normalization test still passes. Nothing else in the suite catches that.
 *
 * Assertions are deliberately attribute-level: jsdom's constraint-validation
 * implementation is not a faithful stand-in for a real browser, so exercising
 * it would prove nothing about the shipped behaviour.
 */

const tauriMocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn((_name: string, _listener: (event: { payload: unknown }) => void) => Promise.resolve(() => {})),
  writeText: vi.fn((_url: string) => Promise.resolve()),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: tauriMocks.invoke,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: tauriMocks.listen,
}));

vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  writeText: tauriMocks.writeText,
}));

// `activeSession` is a derived store (src/lib/stores/sessions.ts:769-770), so it
// cannot be set directly — the component only renders its body when it is
// truthy. Swap it for a writable holding a minimal fake session.
const storeMocks = vi.hoisted(() => ({
  setActiveSession: undefined as ((session: Session | null) => void) | undefined,
}));

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  const activeSession = writable<Session | null>(null);
  storeMocks.setActiveSession = activeSession.set;
  return { activeSession };
});

function fakeSession(id: string): Session {
  return {
    id,
    name: `Session ${id}`,
    session_type: { Solo: { cli: 'claude' } },
    project_path: 'D:/Code Projects/hive-manager',
    state: 'Running',
    created_at: '2026-07-18T12:00:00Z',
    agents: [],
  } as unknown as Session;
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

/** Flushes the component's await continuations plus Svelte's scheduler. */
async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const CLOSED_STATUS = { open: false, docked: false, url: null, session_url: null };
const LIVE_URL = 'http://localhost:5173/';
const LIVE_STATUS = { ...CLOSED_STATUS, open: true, docked: true, url: LIVE_URL };

function emitPreviewEvent(name: string, payload: unknown) {
  const registration = tauriMocks.listen.mock.calls.find(([event]) => event === name);
  if (!registration) throw new Error(`Missing ${name} listener`);
  registration[1]({ payload });
}

async function reopen(container: HTMLElement) {
  const scope = within(container);
  const trigger = scope.getByRole('button', { name: 'Preview' });
  await waitFor(() => expect(trigger.getAttribute('aria-expanded')).toBe('false'));
  await fireEvent.click(trigger);
  return await scope.findByRole('textbox', { name: 'Preview URL' }) as HTMLInputElement;
}

/** Renders the header with `session` active and the preview form expanded. */
async function renderExpanded(session: Session) {
  const view = render(PreviewControl);
  storeMocks.setActiveSession?.(session);

  const toggle = await waitFor(() => {
    const button = view.container.querySelector<HTMLButtonElement>('.preview-toggle');
    if (!button) throw new Error('preview toggle never rendered');
    return button;
  });
  await fireEvent.click(toggle);

  const input = await waitFor(() => {
    const found = view.container.querySelector<HTMLInputElement>('#session-preview-url');
    if (!found) throw new Error('preview URL input never rendered');
    return found;
  });

  return { ...view, input };
}

let popoverStyle: HTMLStyleElement;
beforeEach(() => {
  // JSDOM reports zero-sized anchors. Preserve access to mounted content while
  // exercising Bits' real open state, focus management and Escape handling.
  popoverStyle = document.createElement('style');
  popoverStyle.textContent = '.preview-popover { visibility: visible; }';
  document.head.append(popoverStyle);
});

afterEach(() => {
  cleanup();
  popoverStyle.remove();
  tauriMocks.invoke.mockReset();
  tauriMocks.listen.mockReset();
  tauriMocks.listen.mockImplementation(() => Promise.resolve(() => {}));
  tauriMocks.writeText.mockReset().mockResolvedValue(undefined);
  storeMocks.setActiveSession?.(null);
});

describe('PreviewControl preview URL input', () => {
  it('uses type="text" so scheme-less input survives native constraint validation', async () => {
    tauriMocks.invoke.mockResolvedValue(CLOSED_STATUS);

    const { input } = await renderExpanded(fakeSession('session-a'));

    // The whole of issue #157 section 1 hangs off this one attribute.
    expect(input.getAttribute('type')).toBe('text');
    expect(input.type).toBe('text');

    // The URL keyboard / autofill affordances must survive the type change.
    expect(input.getAttribute('inputmode')).toBe('url');
    expect(input.getAttribute('autocomplete')).toBe('url');

    // The placeholder is the operator-facing promise that the scheme is optional.
    const placeholder = input.getAttribute('placeholder') ?? '';
    expect(placeholder).toContain('localhost:5173');
    expect(placeholder).not.toMatch(/https?:\/\//);
  });

  it('submits a scheme-less URL to the backend verbatim, without pre-normalizing', async () => {
    tauriMocks.invoke.mockResolvedValue(CLOSED_STATUS);

    const { container, input } = await renderExpanded(fakeSession('session-a'));

    await fireEvent.input(input, { target: { value: 'localhost:5173/dashboard' } });

    const form = container.querySelector('.preview-form');
    expect(form).not.toBeNull();
    await fireEvent.submit(form!);

    await waitFor(() => {
      expect(tauriMocks.invoke).toHaveBeenCalledWith('open_preview_window', {
        url: 'localhost:5173/dashboard',
        sessionId: 'session-a',
      });
    });
  });

  it('forwards a scheme-less bare host verbatim too', async () => {
    tauriMocks.invoke.mockResolvedValue(CLOSED_STATUS);

    const { container, input } = await renderExpanded(fakeSession('session-a'));

    await fireEvent.input(input, { target: { value: 'github.com/owner/repo' } });
    await fireEvent.submit(container.querySelector('.preview-form')!);

    await waitFor(() => {
      expect(tauriMocks.invoke).toHaveBeenCalledWith('open_preview_window', {
        url: 'github.com/owner/repo',
        sessionId: 'session-a',
      });
    });
  });
});

describe('PreviewControl preview action freshness', () => {
  it('drops a dock/close status that resolves after the session changed', async () => {
    const staleDock = deferred<unknown>();

    tauriMocks.invoke.mockImplementation((command: string, args?: { sessionId?: string }) => {
      if (command === 'get_preview_status') {
        return Promise.resolve(
          args?.sessionId === 'session-a'
            ? { open: true, docked: false, url: 'http://127.0.0.1:5173/', session_url: null }
            : CLOSED_STATUS,
        );
      }
      if (command === 'dock_preview_window') return staleDock.promise;
      return Promise.resolve(CLOSED_STATUS);
    });

    const { container } = render(PreviewControl);
    storeMocks.setActiveSession?.(fakeSession('session-a'));
    const toggle = await waitFor(() => {
      const button = container.querySelector<HTMLButtonElement>('.preview-toggle');
      if (!button) throw new Error('preview toggle never rendered');
      return button;
    });
    await fireEvent.click(toggle);

    const dockButton = await waitFor(() => {
      const buttons = container.querySelectorAll<HTMLButtonElement>('.preview-action');
      const found = Array.from(buttons).find(
        (button) => button.getAttribute('aria-label') === 'Dock the preview beside Hive Manager',
      );
      if (!found) throw new Error('dock button never rendered');
      return found;
    });

    // Dock request goes out for session-a and stays in flight...
    await fireEvent.click(dockButton);
    // ...while the operator switches to a session that has no remembered URL.
    storeMocks.setActiveSession?.(fakeSession('session-b'));
    await waitFor(() => {
      expect(container.querySelector('.preview-toggle')?.getAttribute('aria-expanded')).toBe('false');
    });

    // The late response carries session-a's URL. It must not be applied.
    staleDock.resolve({
      open: true,
      docked: true,
      url: 'http://127.0.0.1:5173/',
      session_url: 'http://127.0.0.1:5173/',
    });
    await staleDock.promise;

    await fireEvent.click(container.querySelector<HTMLButtonElement>('.preview-toggle')!);
    const input = await waitFor(() => {
      const found = container.querySelector<HTMLInputElement>('#session-preview-url');
      if (!found) throw new Error('preview URL input never rendered');
      return found;
    });

    // Without the freshness gate this is session-a's URL, and submitting it
    // would persist it under session-b's id.
    expect(input.value).toBe('');
  });

  /**
   * Renders the header on session-a with the preview open, fires the dock
   * action, and hands back a handle that rejects it on demand. The two tests
   * below differ only in whether the operator switches sessions first, so the
   * flush timing is identical — that is what makes the negative assertion in
   * the second test meaningful rather than a race that happens to pass.
   */
  async function dockFailureHarness() {
    const staleDock = deferred<unknown>();

    tauriMocks.invoke.mockImplementation((command: string, args?: { sessionId?: string }) => {
      if (command === 'get_preview_status') {
        return Promise.resolve(
          args?.sessionId === 'session-a'
            ? { open: true, docked: false, url: 'http://127.0.0.1:5173/', session_url: null }
            : CLOSED_STATUS,
        );
      }
      if (command === 'dock_preview_window') return staleDock.promise;
      return Promise.resolve(CLOSED_STATUS);
    });

    const { container } = render(PreviewControl);
    storeMocks.setActiveSession?.(fakeSession('session-a'));
    const toggle = await waitFor(() => {
      const button = container.querySelector<HTMLButtonElement>('.preview-toggle');
      if (!button) throw new Error('preview toggle never rendered');
      return button;
    });
    await fireEvent.click(toggle);

    const dockButton = await waitFor(() => {
      const buttons = container.querySelectorAll<HTMLButtonElement>('.preview-action');
      const found = Array.from(buttons).find(
        (button) => button.getAttribute('aria-label') === 'Dock the preview beside Hive Manager',
      );
      if (!found) throw new Error('dock button never rendered');
      return found;
    });
    await fireEvent.click(dockButton);

    return { container, staleDock };
  }

  it('surfaces a dock failure that lands while its own session is still active', async () => {
    const { container, staleDock } = await dockFailureHarness();

    staleDock.reject(new Error('dock failed for session-a'));
    await staleDock.promise.catch(() => {});
    await flush();

    const error = container.querySelector('.preview-error');
    expect(error?.textContent).toContain('dock failed for session-a');
  });

  it('drops a dock/close failure that rejects after the session changed', async () => {
    const { container, staleDock } = await dockFailureHarness();

    // The operator switches away while the dock request is still in flight.
    // Wait on a signal that only session-b can produce: `.preview-toggle`
    // already exists for session-a, so waiting for it can resolve before the
    // switch has been processed at all, leaving the rejection below to race
    // against a header still bound to session-a.
    storeMocks.setActiveSession?.(fakeSession('session-b'));
    await waitFor(() => {
      expect(tauriMocks.invoke).toHaveBeenCalledWith('get_preview_status', {
        sessionId: 'session-b',
      });
    });

    staleDock.reject(new Error('dock failed for session-a'));
    await staleDock.promise.catch(() => {});
    await flush();

    // session-a's failure must not be announced in session-b's header, nor
    // mark session-b's URL input invalid.
    const input = await reopen(container);
    expect(within(container).queryByRole('alert')).toBeNull();
    expect(input.getAttribute('aria-invalid')).toBeFalsy();
  });

  /**
   * `previewBusy` is what disables the dock/pop-out cluster, and the backend
   * treats an open and a dock as two dock transitions. `openPreview` guards on
   * `openingPreview` — which the session-change block DOES reset — while
   * claiming `previewBusy`, which it does not. So a second open can start while
   * the first is still in flight, and an unconditional clear in the first one's
   * `finally` would re-enable the controls mid-transition.
   */
  it('does not let a superseded transition release the newer one’s busy claim', async () => {
    const OPEN_STATUS = {
      open: true,
      docked: false,
      url: 'http://localhost:5173/',
      session_url: 'http://localhost:5173/',
    };
    const staleDock = deferred<unknown>();
    const laterOpen = deferred<unknown>();
    let openCalls = 0;

    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === 'open_preview_window') {
        openCalls += 1;
        // First open settles immediately so the live cluster renders; the
        // second is the newer transition whose claim must survive.
        return openCalls === 1 ? Promise.resolve(OPEN_STATUS) : laterOpen.promise;
      }
      if (command === 'dock_preview_window') return staleDock.promise;
      return Promise.resolve(OPEN_STATUS);
    });

    const { container } = render(PreviewControl);
    storeMocks.setActiveSession?.(fakeSession('session-a'));

    const submitUrl = async (value: string) => {
      const input = container.querySelector<HTMLInputElement>('#session-preview-url')!;
      await fireEvent.input(input, { target: { value } });
      await fireEvent.submit(container.querySelector('.preview-form')!);
    };
    const dockButton = () =>
      Array.from(container.querySelectorAll<HTMLButtonElement>('.preview-action')).find(
        (button) => button.getAttribute('aria-label')?.startsWith('Dock'),
      );

    const toggle = await waitFor(() => {
      const found = container.querySelector<HTMLButtonElement>('.preview-toggle');
      if (!found) throw new Error('toggle never rendered');
      return found;
    });
    await fireEvent.click(toggle);
    await submitUrl('localhost:5173');
    await flush();

    // The live cluster is now rendered and idle.
    await waitFor(() => {
      if (!dockButton()) throw new Error('dock cluster never rendered');
    });
    expect(dockButton()!.disabled).toBe(false);

    // Start a dock, then start an open while it is still in flight. `openPreview`
    // guards on `openingPreview`, NOT on `previewBusy`, so it can take the claim
    // out from under the dock — which is precisely the hole being closed.
    await fireEvent.click(dockButton()!);
    await flush();
    expect(dockButton()!.disabled).toBe(true);

    await submitUrl('localhost:6006');
    await flush();
    expect(openCalls).toBe(2);

    // The superseded dock now settles. An unconditional clear here would
    // re-enable the controls while the open transition is still running.
    staleDock.resolve(OPEN_STATUS);
    await staleDock.promise.catch(() => {});
    await flush();
    expect(dockButton()!.disabled).toBe(true);

    // ...and once the OWNER settles the controls are released rather than
    // stranded — the failure mode a plain request-id gate would introduce.
    laterOpen.resolve(OPEN_STATUS);
    await laterOpen.promise.catch(() => {});
    await flush();
    expect(dockButton()!.disabled).toBe(false);
  });
});


describe('Preview status probe ordering', () => {
  it.each(['preview-navigated', 'preview-status'])('keeps live state from %s and prefills the saved session URL', async (eventName) => {
    const probe = deferred<typeof CLOSED_STATUS | { session_url: string }>();
    tauriMocks.invoke.mockReturnValue(probe.promise);
    const view = await renderExpanded(fakeSession('session-a'));
    emitPreviewEvent(eventName, LIVE_STATUS);
    const saved = 'http://localhost:4173/';
    probe.resolve({ ...CLOSED_STATUS, session_url: saved });
    await probe.promise;
    await flush();

    expect(view.getByText(LIVE_URL)).toBeTruthy();
    expect(view.getByText(eventName === 'preview-status' ? 'Docked' : 'Popped out')).toBeTruthy();
    expect(view.input.value).toBe(saved);
  });

  it('keeps an accepted open response and the typed URL over an older closed probe', async () => {
    const probe = deferred<{ session_url: string }>();
    tauriMocks.invoke.mockImplementation((command: string) =>
      command === 'get_preview_status' ? probe.promise : Promise.resolve(LIVE_STATUS));
    const view = await renderExpanded(fakeSession('session-a'));
    await fireEvent.input(view.input, { target: { value: 'localhost:5173' } });
    await fireEvent.click(view.getByRole('button', { name: 'Open' }));
    await view.findByText('Docked');
    probe.resolve({ ...CLOSED_STATUS, session_url: 'http://localhost:9000/' });
    await probe.promise;
    await flush();

    expect(view.getByText(LIVE_URL)).toBeTruthy();
    expect(view.getByText('Docked')).toBeTruthy();
    expect(view.input.value).toBe('localhost:5173');
    storeMocks.setActiveSession?.(fakeSession('session-b'));
    await flush();
    storeMocks.setActiveSession?.(fakeSession('session-a'));
    expect((await reopen(view.container)).value).toBe('localhost:5173');
  });

  it('keeps a newly selected A free of an earlier A probe after A → B → A', async () => {
    const firstA = deferred<{ session_url: string }>();
    const secondA = deferred<typeof LIVE_STATUS>();
    let aProbes = 0;
    tauriMocks.invoke.mockImplementation((_command: string, args: { sessionId: string }) =>
      args.sessionId === 'session-a' ? (++aProbes === 1 ? firstA.promise : secondA.promise) : Promise.resolve(CLOSED_STATUS));
    const view = await renderExpanded(fakeSession('session-a'));
    storeMocks.setActiveSession?.(fakeSession('session-b'));
    await flush();
    storeMocks.setActiveSession?.(fakeSession('session-a'));
    const input = await reopen(view.container);
    secondA.resolve(LIVE_STATUS);
    await view.findByText('Docked');
    firstA.resolve({ ...CLOSED_STATUS, session_url: 'http://localhost:9000/' });
    await firstA.promise;
    await flush();

    expect(view.getByText(LIVE_URL)).toBeTruthy();
    expect(input.value).toBe('');
  });
});

it.each(['before', 'after'] as const)('releases a successful listener after its peer rejects and it resolves %s unmount', async (timing) => {
  const pending = deferred<() => void>();
  const release = vi.fn();
  tauriMocks.listen.mockImplementation(name => name === 'preview-navigated'
    ? pending.promise : Promise.reject(new Error('listener unavailable')));
  const { unmount } = render(PreviewControl);
  await flush();
  if (timing === 'before') {
    pending.resolve(release);
    await flush();
    expect(release).not.toHaveBeenCalled();
  }
  unmount();
  if (timing === 'after') pending.resolve(release);
  await flush();
  expect(release).toHaveBeenCalledTimes(1);
});

describe('Preview reload and clipboard feedback', () => {
  it.each([
    { name: 'the same session', route: [], showError: true },
    { name: 'B', route: ['session-b'], showError: false },
    { name: 'A → B → A', route: ['session-b', 'session-a'], showError: false },
  ])('scopes a deferred reload failure to $name', async ({ route, showError }) => {
    const reload = deferred<void>();
    tauriMocks.invoke.mockImplementation((command: string) =>
      command === 'reload_preview_window' ? reload.promise : Promise.resolve(LIVE_STATUS));
    const view = await renderExpanded(fakeSession('session-a'));
    const button = () => view.getByRole('button', { name: 'Reload the preview' }) as HTMLButtonElement;
    await fireEvent.click(button());
    expect(button().disabled).toBe(true);
    for (const id of route) {
      storeMocks.setActiveSession?.(fakeSession(id));
      await flush();
    }
    const input = route.length ? await reopen(view.container) : view.input;
    reload.reject(new Error('reload failed'));
    await reload.promise.catch(() => {});
    await flush();

    if (showError) expect(view.getByRole('alert').textContent).toContain('reload failed');
    else expect(view.queryByRole('alert')).toBeNull();
    expect(input.getAttribute('aria-invalid')).toBe(showError ? 'true' : null);
    expect(button().disabled).toBe(false);
  });

  it('does not display delayed clipboard success over a newer URL', async () => {
    const copy = deferred<void>();
    tauriMocks.invoke.mockResolvedValue(LIVE_STATUS);
    tauriMocks.writeText.mockReturnValueOnce(copy.promise);
    const view = await renderExpanded(fakeSession('session-a'));
    await fireEvent.click(view.getByRole('button', { name: 'Copy the current preview URL' }));
    const newerUrl = 'http://localhost:5174/';
    emitPreviewEvent('preview-navigated', { url: newerUrl });
    copy.resolve();
    await copy.promise;
    await flush();

    expect(tauriMocks.writeText).toHaveBeenCalledWith(LIVE_URL);
    expect(view.getByText(newerUrl)).toBeTruthy();
    expect(view.queryByText('Copied')).toBeNull();
  });

  it('drops a clipboard rejection after switching sessions even when the live URL matches', async () => {
    const copy = deferred<void>();
    tauriMocks.invoke.mockResolvedValue(LIVE_STATUS);
    tauriMocks.writeText.mockReturnValueOnce(copy.promise);
    const view = await renderExpanded(fakeSession('session-a'));
    await fireEvent.click(view.getByRole('button', { name: 'Copy the current preview URL' }));
    storeMocks.setActiveSession?.(fakeSession('session-b'));
    await flush();
    const input = await reopen(view.container);
    copy.reject(new Error('clipboard failed'));
    await copy.promise.catch(() => {});
    await flush();

    expect(view.queryByRole('alert')).toBeNull();
    expect(input.getAttribute('aria-invalid')).toBeNull();
  });

  it('clears visible clipboard feedback when the preview navigates', async () => {
    tauriMocks.invoke.mockResolvedValue(LIVE_STATUS);
    const view = await renderExpanded(fakeSession('session-a'));
    await fireEvent.click(view.getByRole('button', { name: 'Copy the current preview URL' }));
    expect(await view.findByText('Copied')).toBeTruthy();
    const newerUrl = 'http://localhost:5174/';
    emitPreviewEvent('preview-status', { ...LIVE_STATUS, url: newerUrl });

    expect(await view.findByText(newerUrl)).toBeTruthy();
    expect(view.queryByText('Copied')).toBeNull();
  });
});

it('opens a named Preview disclosure without advertising a dialog and keeps keyboard behavior', async () => {
  tauriMocks.invoke.mockResolvedValue(CLOSED_STATUS);
  const view = await renderExpanded(fakeSession('session-a'));
  const trigger = view.getByRole('button', { name: 'Preview' });
  const content = view.getByRole('region', { name: 'Preview' });
  expect(trigger.getAttribute('aria-haspopup')).toBe('false');
  expect(trigger.getAttribute('aria-expanded')).toBe('true');
  expect(trigger.getAttribute('aria-controls')).toBe(content.id);
  expect(content.contains(view.input)).toBe(true);
  await waitFor(() => expect(document.activeElement).toBe(view.input));
  await fireEvent.keyDown(view.input, { key: 'Escape' });
  await waitFor(() => expect(view.queryByRole('region', { name: 'Preview' })).toBeNull());
  expect(trigger.getAttribute('aria-expanded')).toBe('false');
});
