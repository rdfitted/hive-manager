// Kept behind a local module so route-scope behavior can be tested with the
// lightweight Svelte Vitest configuration, which does not install SvelteKit aliases.
export { page as routePage } from '$app/stores';
