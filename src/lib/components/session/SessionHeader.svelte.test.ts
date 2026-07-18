import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import type { Session } from '$lib/stores/sessions';
import SessionHeader from './SessionHeader.svelte';

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
  listen: vi.fn(() => Promise.resolve(() => {})),
  writeText: vi.fn(() => Promise.resolve()),
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
// truthy. Swap it for a writable holding a minimal fake session and re-export
// the real `serdeEnumVariantName`, which the header uses for its badges.
const storeMocks = vi.hoisted(() => ({
  setActiveSession: undefined as ((session: Session | null) => void) | undefined,
}));

vi.mock('$lib/stores/sessions', async () => {
  const actual = await vi.importActual<typeof import('$lib/stores/sessions')>(
    '$lib/stores/sessions',
  );
  const { writable } = await import('svelte/store');
  const activeSession = writable<Session | null>(null);
  storeMocks.setActiveSession = activeSession.set;
  return { activeSession, serdeEnumVariantName: actual.serdeEnumVariantName };
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

/** Renders the header with `session` active and the preview form expanded. */
async function renderExpanded(session: Session) {
  const view = render(SessionHeader);
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

afterEach(() => {
  cleanup();
  tauriMocks.invoke.mockReset();
  tauriMocks.listen.mockReset();
  tauriMocks.listen.mockImplementation(() => Promise.resolve(() => {}));
  storeMocks.setActiveSession?.(null);
});

describe('SessionHeader preview URL input', () => {
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

describe('SessionHeader preview action freshness', () => {
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

    const { container } = render(SessionHeader);
    storeMocks.setActiveSession?.(fakeSession('session-a'));

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
      expect(container.querySelector('.preview-toggle')).not.toBeNull();
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

    const { container } = render(SessionHeader);
    storeMocks.setActiveSession?.(fakeSession('session-a'));

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
    storeMocks.setActiveSession?.(fakeSession('session-b'));
    await waitFor(() => {
      expect(container.querySelector('.preview-toggle')).not.toBeNull();
    });

    staleDock.reject(new Error('dock failed for session-a'));
    await staleDock.promise.catch(() => {});
    await flush();

    // session-a's failure must not be announced in session-b's header, nor
    // mark session-b's URL input invalid.
    expect(container.querySelector('.preview-error')).toBeNull();
    expect(container.querySelector('#session-preview-url')?.getAttribute('aria-invalid')).toBeFalsy();
  });
});
