/// <reference types="vite/client" />

/**
 * Ambient declaration for Vite's `?raw` import suffix (string contents of a
 * module). Used by registry.test.ts for the static "no chat-core import" check
 * (issue #127, criterion 4). vite/client already declares `*?raw` but this
 * makes the `.ts?raw` form explicit for svelte-check under bundler resolution.
 */
declare module '*?raw' {
  const content: string;
  export default content;
}
