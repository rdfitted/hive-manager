// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue([]) }));
vi.mock('$app/stores', async () => {
  const { readable } = await import('svelte/store');
  return { page: readable({ url: new URL('http://localhost/') }) };
});
vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    sessions: { ...writable({ sessions: [] }), setActiveSession: vi.fn() },
    activeSession: writable(null),
    activeAgents: writable([]),
    serdeEnumVariantName: () => undefined,
  };
});
vi.mock('./LaunchDialog.svelte', () => ({ default: () => {} }));
vi.mock('./AgentTree.svelte', () => ({ default: () => {} }));
vi.mock('./QueenControls.svelte', () => ({ default: () => {} }));
vi.mock('./ResumeConfirmModal.svelte', () => ({ default: () => {} }));

import SessionSidebar from './SessionSidebar.svelte';
import { layout } from '$lib/stores/layout';

beforeEach(() => {
  if (get(layout).leftCollapsed) layout.toggleLeft();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('SessionSidebar rail toggle', () => {
  it('has a visible button that collapses and expands the sidebar', async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32');
    const view = render(SessionSidebar);
    const toggle = view.getByRole('button', { name: 'Collapse sidebar (Ctrl+B)' });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(toggle.querySelector('svg')).toBeTruthy();

    await fireEvent.click(toggle);
    expect(get(layout).leftCollapsed).toBe(true);
    expect(toggle.getAttribute('aria-label')).toBe('Expand sidebar (Ctrl+B)');

    await fireEvent.click(toggle);
    expect(get(layout).leftCollapsed).toBe(false);
  });

  it('labels the shortcut with ⌘ on macOS', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel');
    const view = render(SessionSidebar);
    expect(view.getByRole('button', { name: 'Collapse sidebar (⌘B)' })).toBeTruthy();
  });
});
