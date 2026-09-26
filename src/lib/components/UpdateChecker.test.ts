// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import UpdateChecker from './UpdateChecker.svelte';

const mocks = vi.hoisted(() => ({
  check: vi.fn(),
  relaunch: vi.fn(),
  invoke: vi.fn(),
  confirm: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-updater', () => ({ check: mocks.check }));
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: mocks.relaunch }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: mocks.confirm }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('$lib/stores/sessions', async () => {
  const actual = await vi.importActual<typeof import('$lib/stores/sessions')>('$lib/stores/sessions');
  return { sessionStateToCellStatus: actual.sessionStateToCellStatus };
});

beforeEach(() => {
  mocks.check.mockReset();
  mocks.relaunch.mockReset();
  mocks.invoke.mockReset();
  mocks.confirm.mockReset();
  mocks.invoke.mockResolvedValue([]);
  mocks.relaunch.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

describe('UpdateChecker', () => {
  it('clears downloading when the install-time check finds no update', async () => {
    mocks.check.mockResolvedValueOnce({ version: '0.55.0' }).mockResolvedValueOnce(null);
    const view = render(UpdateChecker);
    await waitFor(() => expect(view.getByText('Update available: v0.55.0')).toBeTruthy());
    await fireEvent.click(view.getByRole('button', { name: 'Update Now' }));
    await waitFor(() => expect(view.queryByText('Downloading update... 0%')).toBeNull());
    expect(view.getByRole('button', { name: 'Check for updates' }).hasAttribute('disabled')).toBe(false);
    expect(mocks.relaunch).not.toHaveBeenCalled();
  });

  it('shows a production check error', async () => {
    vi.stubEnv('DEV', false);
    mocks.check.mockRejectedValue(new Error('ACL denied'));
    const view = render(UpdateChecker);
    await waitFor(() => expect(view.getByRole('alert').textContent).toContain('Could not check for updates'));
  });

  it('asks before relaunching when a session is active', async () => {
    const update = { version: '0.55.0', downloadAndInstall: vi.fn().mockResolvedValue(undefined) };
    mocks.check.mockResolvedValue(update);
    mocks.invoke.mockResolvedValue([{ state: 'Running' }, { state: 'Completed' }]);
    mocks.confirm.mockResolvedValue(false);
    const view = render(UpdateChecker);
    await waitFor(() => expect(view.getByText('Update available: v0.55.0')).toBeTruthy());
    await fireEvent.click(view.getByRole('button', { name: 'Update Now' }));
    await waitFor(() => expect(mocks.confirm).toHaveBeenCalled());
    expect(mocks.confirm.mock.calls[0][0]).toContain('1 session is still active');
    expect(mocks.invoke).toHaveBeenCalledWith('list_sessions');
    expect(mocks.relaunch).not.toHaveBeenCalled();
    expect(view.getByRole('button', { name: 'Restart to finish update' })).toBeTruthy();
    mocks.confirm.mockResolvedValue(true);
    await fireEvent.click(view.getByRole('button', { name: 'Restart to finish update' }));
    await waitFor(() => expect(mocks.relaunch).toHaveBeenCalledTimes(1));
  });

  it('does not check again while an installed update awaits restart', async () => {
    let periodicCheck: (() => void) | undefined;
    const setInterval = window.setInterval.bind(window);
    vi.spyOn(window, 'setInterval').mockImplementation((callback, delay, ...args) => {
      if (delay === 6 * 60 * 60 * 1000) {
        periodicCheck = callback as () => void;
        return 1;
      }
      return setInterval(callback, delay, ...args);
    });
    const update = { version: '0.55.0', downloadAndInstall: vi.fn().mockResolvedValue(undefined) };
    mocks.check.mockResolvedValue(update);
    mocks.invoke.mockResolvedValue([{ state: 'Running' }]);
    mocks.confirm.mockResolvedValue(false);
    const view = render(UpdateChecker);
    await waitFor(() => expect(view.getByText('Update available: v0.55.0')).toBeTruthy());
    await fireEvent.click(view.getByRole('button', { name: 'Update Now' }));
    await waitFor(() => expect(view.getByRole('button', { name: 'Restart to finish update' })).toBeTruthy());

    expect(mocks.check).toHaveBeenCalledTimes(2);
    expect(periodicCheck).toBeDefined();
    periodicCheck?.();
    await Promise.resolve();
    expect(mocks.check).toHaveBeenCalledTimes(2);
    expect(view.queryByText('Update available: v0.55.0')).toBeNull();
    expect(view.getByRole('button', { name: 'Restart to finish update' })).toBeTruthy();
  });
});
