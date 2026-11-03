declare global {
  interface Window {
    __HIVE_INSTANCE__?: Readonly<{ isolated: true; apiBase: string }>;
  }
}

export function isIsolatedInstance(): boolean {
  return typeof window !== 'undefined' && window.__HIVE_INSTANCE__?.isolated === true;
}

const DEFAULT_API_BASE = 'http://localhost:18800';

function resolveApiBase(): string {
  if (isIsolatedInstance()) {
    const base = new URL(window.__HIVE_INSTANCE__!.apiBase);
    if (base.protocol !== 'http:' || base.hostname !== '127.0.0.1'
      || Number(base.port) < 1024 || base.port === '18800'
      || base.username || base.password || base.pathname !== '/' || base.search || base.hash) {
      throw new Error('Invalid isolated instance API base');
    }
    return base.origin;
  }
  const envBase = import.meta.env.VITE_API_BASE;
  if (typeof envBase === 'string' && envBase.length > 0) return envBase;
  return DEFAULT_API_BASE;
}

export const API_BASE = resolveApiBase();

export function apiUrl(path: string): string {
  const base = API_BASE.endsWith('/') ? API_BASE : `${API_BASE}/`;
  const url = new URL(path.replace(/^\/+/, ''), base);
  if (isIsolatedInstance() && url.origin !== new URL(base).origin) {
    throw new Error('Isolated instance API request escaped its origin');
  }
  return url.toString();
}
