// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { isMacPlatform, modifierKey, shortcutLabel } from './platform';

afterEach(() => vi.restoreAllMocks());

describe('platform shortcut labels', () => {
  it('uses ⌘ on macOS', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel');
    expect(isMacPlatform()).toBe(true);
    expect(modifierKey()).toBe('⌘');
    expect(shortcutLabel('B')).toBe('⌘B');
  });

  it('uses Ctrl on Windows', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32');
    expect(isMacPlatform()).toBe(false);
    expect(modifierKey()).toBe('Ctrl');
    expect(shortcutLabel('J')).toBe('Ctrl+J');
  });
});
