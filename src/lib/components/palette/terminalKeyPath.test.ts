// @vitest-environment jsdom
import source from '../Terminal.svelte?raw';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Extract the production handler's palette prefix: importing Terminal.svelte would
// replace the very xterm implementation whose propagation this scout must prove.
const prefix = source.split('term.attachCustomKeyEventHandler((event) => {')[1].split('// Ctrl+F opens the find bar')[0];
const customHandler = new Function('event', `${prefix}\nreturn true;`) as (event: KeyboardEvent) => boolean;

beforeEach(() => {
  vi.stubGlobal('matchMedia', vi.fn(() => ({ matches: false, addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {} })));
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe('production Terminal palette pass-through', () => {
  it.each(['ctrlKey', 'metaKey'] as const)('returns false only for the palette chord (%s)', (modifier) => {
    expect(customHandler(new KeyboardEvent('keydown', { key: 'P', [modifier]: true, shiftKey: true }))).toBe(false);
    expect(customHandler(new KeyboardEvent('keydown', { key: 'k', [modifier]: true }))).toBe(true);
    expect(customHandler(new KeyboardEvent('keydown', { key: 'p', [modifier]: true }))).toBe(true);
    expect(customHandler(new KeyboardEvent('keyup', { key: 'P', [modifier]: true, shiftKey: true }))).toBe(true);
  });

  it.each(['ctrlKey', 'metaKey'] as const)('real xterm lets %s+Shift+P reach the window bubble listener', async (modifier) => {
    const { Terminal } = await import('@xterm/xterm');
    const term = new Terminal();
    const host = document.createElement('div');
    document.body.append(host);
    const seen = vi.fn();
    window.addEventListener('keydown', seen);
    try {
      term.open(host);
      term.attachCustomKeyEventHandler(customHandler);
      const data = vi.fn();
      term.onData(data);
      const event = new KeyboardEvent('keydown', { key: 'P', keyCode: 80, [modifier]: true, shiftKey: true, bubbles: true, cancelable: true });
      term.textarea!.dispatchEvent(event);
      expect(seen).toHaveBeenCalledOnce();
      expect(data).not.toHaveBeenCalled();
      expect(event.defaultPrevented).toBe(false);
    } finally {
      window.removeEventListener('keydown', seen);
      term.dispose();
      host.remove();
    }
  });
});
