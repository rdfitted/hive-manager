/**
 * Platform-aware labels for the app-level shortcuts (Ctrl on Windows/Linux, ⌘ on macOS).
 *
 * The keydown handlers accept either modifier, but a focused xterm consumes Ctrl+<letter> and
 * forwards it to the agent, so on macOS the ⌘ form is the one that works everywhere.
 */
export function isMacPlatform(): boolean {
  if (typeof navigator === 'undefined') return false;
  return /mac/i.test(navigator.platform || navigator.userAgent);
}

/** Name of the shortcut modifier key: `⌘` on macOS, `Ctrl` elsewhere. */
export function modifierKey(): string {
  return isMacPlatform() ? '⌘' : 'Ctrl';
}

/** A modifier+key label for tooltips: `⌘B` on macOS, `Ctrl+B` elsewhere. */
export function shortcutLabel(key: string): string {
  return isMacPlatform() ? `⌘${key}` : `Ctrl+${key}`;
}
