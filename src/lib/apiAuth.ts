import { invoke, isTauri } from '@tauri-apps/api/core';
import { API_BASE } from './config';

/**
 * Per-launch bearer token for the local HTTP API (see src-tauri/src/http/auth.rs).
 * The main window reads it over the `get_api_token` Tauri command; every fetch
 * to the API origin then carries `Authorization: Bearer <token>`. EventSource
 * cannot set headers, so stream URLs carry it as `hive_api_token`.
 */
export const STREAM_TOKEN_QUERY = 'hive_api_token';

let token: string | null = null;
let ready: Promise<void> | null = null;

export function loadApiToken(): Promise<void> {
  if (!ready) {
    ready = (async () => {
      if (!isTauri()) return;
      try {
        token = await invoke<string>('get_api_token');
      } catch {
        // Let the next request retry instead of caching the failure for the page's lifetime.
        token = null;
        ready = null;
      }
    })();
  }
  return ready;
}

export function apiToken(): string | null {
  return token;
}

function requestUrl(input: RequestInfo | URL): string {
  if (typeof input === 'string') return input;
  if (input instanceof URL) return input.href;
  return input.url;
}

function isApiRequest(url: string): boolean {
  try {
    const base = typeof window !== 'undefined' ? window.location.href : API_BASE;
    return new URL(url, base).origin === new URL(API_BASE).origin;
  } catch {
    return false;
  }
}

type AuthorizedFetch = typeof fetch & { __hiveApiAuth?: true };

/** Wrap `fetch` once so every API request is authorized. Other origins pass through untouched. */
export function installApiAuth(target: { fetch: typeof fetch } = globalThis): void {
  const current = target.fetch as AuthorizedFetch;
  if (current.__hiveApiAuth) return;
  const original = current.bind(target);
  const wrapped: AuthorizedFetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    if (!isApiRequest(requestUrl(input))) return original(input, init);
    await loadApiToken();
    if (!token) return original(input, init);
    const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined));
    if (!headers.has('Authorization')) headers.set('Authorization', `Bearer ${token}`);
    return original(input, { ...init, headers });
  };
  wrapped.__hiveApiAuth = true;
  target.fetch = wrapped;
}

/** Add the token to an EventSource URL; unchanged when no token is available. */
export function authorizedStreamUrl(url: string): string {
  if (!token) return url;
  const parsed = new URL(url);
  parsed.searchParams.set(STREAM_TOKEN_QUERY, token);
  return parsed.toString();
}

/** Test-only reset. */
export function __resetApiAuthForTests(next: string | null = null): void {
  token = next;
  ready = next === null ? null : Promise.resolve();
}
