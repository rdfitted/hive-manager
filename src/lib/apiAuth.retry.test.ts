import { expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { apiToken, loadApiToken } from './apiAuth';

vi.mock('@tauri-apps/api/core', () => ({
  isTauri: () => true,
  invoke: vi.fn(),
}));

it('retries get_api_token after a failed load instead of caching the failure', async () => {
  const mocked = vi.mocked(invoke);
  mocked.mockRejectedValueOnce(new Error('not ready'));
  await loadApiToken();
  expect(apiToken()).toBeNull();

  mocked.mockResolvedValueOnce('tok-after-retry');
  await loadApiToken();
  expect(apiToken()).toBe('tok-after-retry');
  expect(mocked).toHaveBeenCalledTimes(2);
});
