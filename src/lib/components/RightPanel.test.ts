// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import { get } from 'svelte/store';

vi.mock('./StatusPanel.svelte', () => ({ default: () => {} }));
vi.mock('./PlanView.svelte', () => ({ default: () => {} }));
vi.mock('./workgraph/WorkGraphView.svelte', () => ({ default: () => {} }));
vi.mock('./CoordinationPanel.svelte', () => ({ default: () => {} }));
vi.mock('./ConversationViewer.svelte', () => ({ default: () => {} }));
vi.mock('./timeline/TimelineView.svelte', () => ({ default: () => {} }));
vi.mock('./SessionFilesView.svelte', () => ({ default: () => {} }));

import RightPanel from './RightPanel.svelte';
import { layout } from '$lib/stores/layout';

beforeEach(() => {
  if (get(layout).rightCollapsed) layout.toggleRight();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('RightPanel rail toggle', () => {
  it('has a visible button that collapses and expands the panel', async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32');
    const view = render(RightPanel);
    const toggle = view.getByRole('button', { name: 'Collapse panel (Ctrl+J)' });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(toggle.querySelector('svg')).toBeTruthy();

    await fireEvent.click(toggle);
    expect(get(layout).rightCollapsed).toBe(true);
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(toggle.getAttribute('aria-label')).toBe('Expand panel (Ctrl+J)');

    await fireEvent.click(toggle);
    expect(get(layout).rightCollapsed).toBe(false);
  });

  it('labels the shortcut with ⌘ on macOS', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel');
    const view = render(RightPanel);
    expect(view.getByRole('button', { name: 'Collapse panel (⌘J)' })).toBeTruthy();
  });
});
