// Test-only stand-in for SvelteKit's `$app/navigation`, which the plain svelte() Vitest config
// (see vitest.config.ts) cannot resolve. Tests can still override it with vi.mock('$app/navigation').
export async function goto(_url: string | URL, _opts?: Record<string, unknown>): Promise<void> {}
