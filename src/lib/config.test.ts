// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';

afterEach(() => {
  delete window.__HIVE_INSTANCE__;
  vi.unstubAllEnvs();
  vi.resetModules();
});

describe('instance API base', () => {
  it('uses the injected API before build-time settings', async () => {
    vi.stubEnv('VITE_API_BASE', 'http://localhost:18800');
    window.__HIVE_INSTANCE__ = { isolated: true, apiBase: 'http://127.0.0.1:18891' };
    const config = await import('./config');
    expect(config.API_BASE).toBe('http://127.0.0.1:18891');
    expect(config.apiUrl('/api/sessions')).toBe('http://127.0.0.1:18891/api/sessions');
    expect(() => config.apiUrl('http://localhost:18800/api/sessions')).toThrow('escaped its origin');
  });

  it('keeps the normal default and environment override', async () => {
    vi.stubEnv('VITE_API_BASE', '');
    expect((await import('./config')).API_BASE).toBe('http://localhost:18800');
    vi.resetModules();
    vi.stubEnv('VITE_API_BASE', 'http://localhost:19999');
    expect((await import('./config')).API_BASE).toBe('http://localhost:19999');
  });

  it.each(['http://127.0.0.1:18800', 'http://localhost:18891', 'https://127.0.0.1:18891', 'http://127.0.0.1', 'http://127.0.0.1:18891/api'])
    ('refuses an unsafe isolated API base: %s', async (apiBase) => {
      window.__HIVE_INSTANCE__ = { isolated: true, apiBase };
      await expect(import('./config')).rejects.toThrow('Invalid isolated instance API base');
    });
});
