// Test-only stand-in for SvelteKit's `$app/stores`, which the plain svelte() Vitest config
// (see vitest.config.ts) cannot resolve. Tests can still override it with vi.mock('$app/stores').
import { readable } from 'svelte/store';

export const page = readable({ url: new URL('http://localhost/'), params: {}, route: { id: '/' } });
export const navigating = readable(null);
export const updated = readable(false);
