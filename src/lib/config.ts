const DEFAULT_API_BASE = 'http://localhost:18800';

function resolveApiBase(): string {
  const envBase = import.meta.env.VITE_API_BASE;
  if (typeof envBase === 'string' && envBase.length > 0) {
    return envBase;
  }

  if (typeof window !== 'undefined' && /^https?:/.test(window.location.origin)) {
    return window.location.origin;
  }

  return DEFAULT_API_BASE;
}

export const API_BASE = resolveApiBase();

export function apiUrl(path: string): string {
  return new URL(path, API_BASE).toString();
}
