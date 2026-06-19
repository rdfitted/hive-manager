import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { svelteTesting } from '@testing-library/svelte/vite';
import path from 'node:path';

/**
 * Dedicated Vitest config (issue #127).
 *
 * Uses the PLAIN `svelte()` plugin with `configFile: false` instead of the full
 * `sveltekit()` plugin. SvelteKit's pipeline runs Vite's CSS preprocessor during
 * Svelte compile, which throws "Cannot create proxy with a non-object" under
 * Vitest. Skipping svelte.config.js (and thus vitePreprocess) avoids that — the
 * jsdom component test only asserts DOM structure, so CSS preprocessing is
 * irrelevant here.
 *
 * Default environment is `node` so the pure-TS registry test stays DOM-free and
 * fast; files matching the svelte-test glob run in jsdom via the per-file
 * override. `svelteTesting()` adds the `browser` resolve condition + auto-cleanup.
 *
 * Vitest prefers vitest.config.ts over vite.config.js; the `test` block in
 * vite.config.js documents the same intent for the SvelteKit-native tooling.
 */
export default defineConfig({
  plugins: [svelte({ configFile: false, compilerOptions: { dev: true } }), svelteTesting()],
  resolve: {
    alias: {
      $lib: path.resolve(__dirname, './src/lib'),
    },
  },
  test: {
    environment: 'node',
    environmentMatchGlobs: [['**/*.svelte.test.ts', 'jsdom']],
    include: ['src/**/*.{test,spec}.{js,ts}'],
    globals: false,
  },
});
