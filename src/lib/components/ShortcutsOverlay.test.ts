// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import ShortcutsOverlay from './ShortcutsOverlay.svelte';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function layoutKeys(container: HTMLElement): Array<string | null> {
  const row = Array.from(container.querySelectorAll('li'))
    .find((li) => li.textContent?.includes('Toggle left sidebar'));
  return Array.from(row?.querySelectorAll('kbd') ?? []).map((kbd) => kbd.textContent);
}

describe('ShortcutsOverlay', () => {
  it('shows ⌘ for the layout toggles on macOS', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel');
    const view = render(ShortcutsOverlay, { open: true, onClose: () => {} });
    expect(layoutKeys(view.container)).toEqual(['⌘', 'B']);
    expect(view.getByText('Toggle left sidebar')).toBeTruthy();
  });

  it('shows Ctrl and says a focused terminal keeps it, elsewhere', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32');
    const view = render(ShortcutsOverlay, { open: true, onClose: () => {} });
    expect(layoutKeys(view.container)).toEqual(['Ctrl', 'B']);
    expect(view.getByText('Toggle left sidebar (not while a terminal has focus)')).toBeTruthy();
  });
});
