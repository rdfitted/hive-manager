// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import UpdateChecker from './UpdateChecker.svelte';

const mocks = vi.hoisted(() => ({
  check: vi.fn(),
  relaunch: vi.fn(),
  invoke: vi.fn(),
  confirm: vi.fn(),
  getVersion: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-updater', () => ({ check: mocks.check }));
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: mocks.relaunch }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: mocks.getVersion }));
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
  mocks.getVersion.mockReset();
  mocks.getVersion.mockResolvedValue('0.55.1');
  mocks.invoke.mockResolvedValue([]);
  mocks.relaunch.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

describe('UpdateChecker', () => {
  it('shows the installed version next to the update check', async () => {
    mocks.check.mockResolvedValue(null);
    const view = render(UpdateChecker);
    await waitFor(() => expect(view.getByText('v0.55.1')).toBeTruthy());
  });

  it('confirms up to date only after a manual check', async () => {
    mocks.check.mockResolvedValue(null);
    const view = render(UpdateChecker);
    await waitFor(() => expect(mocks.check).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(view.queryByText('Checking for updates...')).toBeNull());
    await waitFor(() => expect(view.getByText('v0.55.1')).toBeTruthy());
    expect(view.queryByText('Up to date')).toBeNull();
    await fireEvent.click(view.getByRole('button', { name: 'Check for updates' }));
    await waitFor(() => expect(view.getByRole('status').textContent).toBe('Up to date'));
    expect(mocks.check).toHaveBeenCalledTimes(2);
  });

  it('names the installed version in the update banner', async () => {
    mocks.check.mockResolvedValue({ version: '0.56.0', close: vi.fn().mockResolvedValue(undefined) });
    const view = render(UpdateChecker);
    await waitFor(() => expect(view.getByText('Update available: v0.56.0')).toBeTruthy());
    await waitFor(() => expect(view.getByText('Installed: v0.55.1')).toBeTruthy());
  });

  it('renders without a version when the app version is unavailable', async () => {
    mocks.getVersion.mockRejectedValue(new Error('not in Tauri'));
    mocks.check.mockResolvedValue(null);
    const view = render(UpdateChecker);
    await waitFor(() => expect(mocks.getVersion).toHaveBeenCalled());
    expect(view.queryByText(/^v\d/)).toBeNull();
    expect(view.getByRole('button', { name: 'Check for updates' })).toBeTruthy();
  });

  it('clears downloading when the install-time check finds no update', async () => {
    const close = vi.fn().mockResolvedValue(undefined);
    mocks.check.mockResolvedValueOnce({ version: '0.55.0', close }).mockResolvedValueOnce(null);
    const view = render(UpdateChecker);
    await waitFor(() => expect(view.getByText('Update available: v0.55.0')).toBeTruthy());
    await fireEvent.click(view.getByRole('button', { name: 'Update Now' }));
    await waitFor(() => expect(view.queryByText('Downloading update... 0%')).toBeNull());
    expect(view.getByRole('button', { name: 'Check for updates' }).hasAttribute('disabled')).toBe(false);
    expect(close).toHaveBeenCalledTimes(1);
    expect(mocks.relaunch).not.toHaveBeenCalled();
  });

  it('shows a production check error', async () => {
    vi.stubEnv('DEV', false);
    mocks.check.mockRejectedValue(new Error('ACL denied'));
    const view = render(UpdateChecker);
    await waitFor(() => expect(view.getByRole('alert').textContent).toContain('Could not check for updates'));
  });

  it('asks before download and closes the update when an active session declines', async () => {
    const metadata = { version: '0.55.0', close: vi.fn().mockResolvedValue(undefined) };
    const update = { version: '0.55.0', close: vi.fn().mockResolvedValue(undefined), downloadAndInstall: vi.fn().mockResolvedValue(undefined) };
    mocks.check.mockResolvedValueOnce(metadata).mockResolvedValueOnce(update);
    mocks.invoke.mockResolvedValue([{ state: 'Running' }, { state: 'Completed' }]);
    mocks.confirm.mockResolvedValue(false);
    const view = render(UpdateChecker);
    await waitFor(() => expect(view.getByText('Update available: v0.55.0')).toBeTruthy());
    await fireEvent.click(view.getByRole('button', { name: 'Update Now' }));
    await waitFor(() => expect(mocks.confirm).toHaveBeenCalled());
    await waitFor(() => expect(update.close).toHaveBeenCalledTimes(1));
    expect(mocks.confirm.mock.calls[0][0]).toContain('1 session is still active');
    expect(mocks.invoke).toHaveBeenCalledWith('list_sessions');
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
    expect(metadata.close).toHaveBeenCalledTimes(1);
    expect(update.close).toHaveBeenCalledTimes(1);
    expect(mocks.relaunch).not.toHaveBeenCalled();
    expect(view.queryByRole('button', { name: 'Restart to finish update' })).toBeNull();
    expect(view.getByRole('button', { name: 'Update Now' })).toBeTruthy();
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
    const metadata = { version: '0.55.0', close: vi.fn().mockResolvedValue(undefined) };
    const update = { version: '0.55.0', close: vi.fn().mockResolvedValue(undefined), downloadAndInstall: vi.fn().mockResolvedValue(undefined) };
    mocks.check.mockResolvedValueOnce(metadata).mockResolvedValueOnce(update);
    mocks.invoke.mockResolvedValue([{ state: 'Running' }]);
    mocks.confirm.mockResolvedValue(true);
    const view = render(UpdateChecker);
    await waitFor(() => expect(view.getByText('Update available: v0.55.0')).toBeTruthy());
    await fireEvent.click(view.getByRole('button', { name: 'Update Now' }));
    await waitFor(() => expect(view.getByRole('button', { name: 'Restart to finish update' })).toBeTruthy());

    expect(mocks.check).toHaveBeenCalledTimes(2);
    expect(update.downloadAndInstall).toHaveBeenCalledTimes(1);
    expect(mocks.relaunch).toHaveBeenCalledTimes(1);
    expect(periodicCheck).toBeDefined();
    periodicCheck?.();
    await Promise.resolve();
    expect(mocks.check).toHaveBeenCalledTimes(2);
    expect(view.queryByText('Update available: v0.55.0')).toBeNull();
    expect(view.getByRole('button', { name: 'Restart to finish update' })).toBeTruthy();
  });
});
