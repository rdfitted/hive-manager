import { afterEach, describe, expect, it, vi } from 'vitest';
import { __resetApiAuthForTests, authorizedStreamUrl, installApiAuth, STREAM_TOKEN_QUERY } from './apiAuth';
import { apiUrl } from './config';

vi.mock('@tauri-apps/api/core', () => ({
  isTauri: () => false,
  invoke: vi.fn(),
}));

function fakeTarget() {
  const calls: Array<{ input: RequestInfo | URL; init?: RequestInit }> = [];
  const target = {
    fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
      calls.push({ input, init });
      return new Response('{}');
    }) as typeof fetch,
  };
  installApiAuth(target);
  return { target, calls };
}

function authorization(init?: RequestInit): string | null {
  return new Headers(init?.headers).get('Authorization');
}

afterEach(() => __resetApiAuthForTests(null));

describe('apiAuth', () => {
  it('adds the bearer header to API requests only', async () => {
    __resetApiAuthForTests('tok123');
    const { target, calls } = fakeTarget();
    await target.fetch(apiUrl('/api/sessions'), { headers: { 'Content-Type': 'application/json' } });
    await target.fetch('https://example.com/api/sessions');
    expect(authorization(calls[0].init)).toBe('Bearer tok123');
    expect(new Headers(calls[0].init?.headers).get('Content-Type')).toBe('application/json');
    expect(authorization(calls[1].init)).toBeNull();
  });

  it('keeps an explicit Authorization header', async () => {
    __resetApiAuthForTests('tok123');
    const { target, calls } = fakeTarget();
    await target.fetch(apiUrl('/api/sessions'), { headers: { Authorization: 'Bearer other' } });
    expect(authorization(calls[0].init)).toBe('Bearer other');
  });

  it('passes requests through unchanged without a token', async () => {
    const { target, calls } = fakeTarget();
    await target.fetch(apiUrl('/api/sessions'));
    expect(calls[0].init).toBeUndefined();
  });

  it('installs once', () => {
    const { target } = fakeTarget();
    const wrapped = target.fetch;
    installApiAuth(target);
    expect(target.fetch).toBe(wrapped);
  });

  it('adds the token to stream URLs', () => {
    const url = apiUrl('/api/sessions/s/stream');
    expect(authorizedStreamUrl(url)).toBe(url);
    __resetApiAuthForTests('tok123');
    expect(new URL(authorizedStreamUrl(url)).searchParams.get(STREAM_TOKEN_QUERY)).toBe('tok123');
  });
});
