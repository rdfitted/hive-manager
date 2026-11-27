import { installApiAuth, loadApiToken } from '$lib/apiAuth';

// Tauri doesn't have a Node.js server to do proper SSR
// so we use adapter-static with a fallback to index.html to put the site in SPA mode
// See: https://svelte.dev/docs/kit/single-page-apps
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
export const ssr = false;

// Every local-API request needs the per-launch bearer token. Install the fetch
// wrapper before any store fetches, and hold the first render until the token
// is known so EventSource URLs (which cannot carry headers) include it.
installApiAuth();

export const load = async () => {
  await loadApiToken();
  return {};
};
