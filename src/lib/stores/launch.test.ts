import { describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { createLaunchStore } from './launch';
import { sessions, type HiveLaunchConfig } from './sessions';

vi.mock('./sessions', () => ({
  sessions: {
    launchHiveV2: vi.fn(), launchResearch: vi.fn(), launchFusion: vi.fn(),
    launchSolo: vi.fn(), launchDebate: vi.fn(),
  },
}));

describe('launch lifecycle and requests', () => {
  it('issues increasing ids for repeated requests and keeps defaults separate from form fields', () => {
    const store = createLaunchStore();
    store.setDefaultProjectPath('D:/project');
    store.open('fusion');
    const first = get(store);
    store.open('fusion');
    expect(get(store).requestId).toBeGreaterThan(first.requestId);
    expect(get(store)).toEqual({
      requestId: 2, requestedMode: 'fusion', landingOpen: true,
      launching: false, launchError: '', defaultProjectPath: 'D:/project',
    });
    store.open();
    expect(get(store).requestedMode).toBeUndefined();
  });

  it.each([
    ['launchHive', 'launchHiveV2'], ['launchResearch', 'launchResearch'],
    ['launchFusion', 'launchFusion'], ['launchSolo', 'launchSolo'], ['launchDebate', 'launchDebate'],
  ] as const)('%s forwards its payload and maintains pending/success lifecycle', async (handler, method) => {
    const store = createLaunchStore();
    const config = { project_path: 'D:/project', prompt: 'Do the task' };
    let finish!: () => void;
    vi.mocked(sessions[method]).mockImplementationOnce(() => new Promise((resolve) => {
      finish = () => resolve(undefined as never);
    }));
    store.open();
    const pending = store[handler](config as never);
    expect(get(store).launching).toBe(true);
    expect(get(store).launchError).toBe('');
    expect(sessions[method]).toHaveBeenLastCalledWith(config);
    store.close();
    expect(get(store).landingOpen).toBe(false);
    finish();
    await pending;
    expect(get(store).launching).toBe(false);
    expect(get(store).landingOpen).toBe(false);
  });

  it('retains a failed request, clears errors explicitly, and allows dismissal', async () => {
    const store = createLaunchStore();
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.mocked(sessions.launchHiveV2).mockRejectedValueOnce(new Error('Launch unavailable'));
    store.open('hive');
    await store.launchHive({} as HiveLaunchConfig);
    expect(get(store)).toMatchObject({ launching: false, landingOpen: true, launchError: 'Error: Launch unavailable' });
    store.clearError();
    expect(get(store).launchError).toBe('');
    store.close();
    expect(get(store).landingOpen).toBe(false);
    error.mockRestore();
  });
});
