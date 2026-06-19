import { beforeEach, describe, it, expect } from 'vitest';
import {
  registerToolRenderer,
  resolveToolRenderer,
  clearCustomRenderers,
  clearAllRenderers,
  rendererCount,
  type ToolRendererComponent,
} from './registry';
// Vite `?raw` import: load registry.ts as a string for the static no-chat-core
// dependency check (criterion 4) without pulling in node builtins.
import registrySource from './registry.ts?raw';

// A throwaway stand-in for a Svelte component. Resolution never invokes it, so
// the registry can be exercised entirely in vitest's node env (no DOM, no Svelte
// runtime). This is the core of criterion 4: registry.ts has zero chat-core
// imports and resolves purely from data.
const fakeComponent = (() => {}) as unknown as ToolRendererComponent;

describe('tool-render registry', () => {
  beforeEach(() => {
    // Reset to a known-good state: clear everything, then register the four
    // built-in ids (without importing the .svelte widgets, which would drag in
    // the Svelte runtime under node).
    clearAllRenderers();
    for (const id of ['table', 'diff', 'approval', 'chart']) {
      registerToolRenderer({ id, component: fakeComponent });
    }
  });

  it('resolves an explicit renderer hint to the matching built-in id', () => {
    expect(resolveToolRenderer({ renderer: 'diff' })?.id).toBe('diff');
    expect(resolveToolRenderer({ renderer: 'table' })?.id).toBe('table');
    expect(resolveToolRenderer({ renderer: 'approval' })?.id).toBe('approval');
  });

  it('falls back to null for an unknown renderer hint', () => {
    // Drives ToolRenderHost's JSON <pre> fallback (criterion 3).
    expect(resolveToolRenderer({ renderer: 'future-widget' })).toBeNull();
  });

  it('returns null when nothing is provided', () => {
    expect(resolveToolRenderer({})).toBeNull();
  });

  it('matches a custom renderer by string toolName', () => {
    registerToolRenderer({ id: 'gitdiff', match: 'git-diff', component: fakeComponent });
    expect(resolveToolRenderer({ toolName: 'git-diff' })?.id).toBe('gitdiff');
  });

  it('matches a custom renderer by string against the renderer hint', () => {
    registerToolRenderer({ id: 'special', match: 'special-hint', component: fakeComponent });
    expect(resolveToolRenderer({ renderer: 'special-hint' })?.id).toBe('special');
  });

  it('matches a custom renderer via a predicate', () => {
    registerToolRenderer({
      id: 'shellish',
      match: (input) => (input.toolName ?? '').startsWith('shell'),
      component: fakeComponent,
    });
    expect(resolveToolRenderer({ toolName: 'shell-table' })?.id).toBe('shellish');
    expect(resolveToolRenderer({ toolName: 'other' })).toBeNull();
  });

  it('orders multiple matching custom renderers by descending priority', () => {
    registerToolRenderer({
      id: 'low',
      match: () => true,
      priority: 1,
      component: fakeComponent,
    });
    registerToolRenderer({
      id: 'high',
      match: () => true,
      priority: 10,
      component: fakeComponent,
    });
    expect(resolveToolRenderer({ toolName: 'anything' })?.id).toBe('high');
  });

  it('prefers an exact built-in id hint over a matching custom predicate', () => {
    // A custom renderer that claims everything must NOT shadow a direct
    // built-in id hit (priority a/b beats c).
    registerToolRenderer({
      id: 'greedy',
      match: () => true,
      priority: 999,
      component: fakeComponent,
    });
    expect(resolveToolRenderer({ renderer: 'diff' })?.id).toBe('diff');
  });

  it('clearCustomRenderers removes custom renderers but keeps built-ins', () => {
    registerToolRenderer({ id: 'temp', match: 'temp', component: fakeComponent });
    expect(resolveToolRenderer({ toolName: 'temp' })?.id).toBe('temp');
    clearCustomRenderers();
    expect(resolveToolRenderer({ toolName: 'temp' })).toBeNull();
    // Built-ins survive.
    expect(resolveToolRenderer({ renderer: 'diff' })?.id).toBe('diff');
  });

  it('registering a custom renderer needs no chat-core import (criterion 4)', () => {
    // This test file imports ONLY ./registry — never ConversationViewer or
    // conversations. Registering + resolving works in isolation, proving a new
    // renderer requires zero chat-core changes.
    const before = rendererCount();
    registerToolRenderer({ id: 'standalone', match: 'standalone', component: fakeComponent });
    expect(rendererCount()).toBe(before + 1);
    expect(resolveToolRenderer({ toolName: 'standalone' })?.id).toBe('standalone');
  });
});

describe('registry.ts has no chat-core dependency (criterion 4, static)', () => {
  it('registry source imports nothing from the chat core', () => {
    // No import of the chat-core files. The only permitted import is `svelte`
    // for the (type-only) Component type.
    expect(registrySource).not.toMatch(/from\s+['"][^'"]*ConversationViewer/);
    expect(registrySource).not.toMatch(/from\s+['"][^'"]*stores\/conversations/);
    expect(registrySource).not.toMatch(/from\s+['"][^'"]*ToolRenderHost/);
    // And confirm it does import the svelte type (sanity).
    expect(registrySource).toMatch(/from\s+['"]svelte['"]/);
  });
});
