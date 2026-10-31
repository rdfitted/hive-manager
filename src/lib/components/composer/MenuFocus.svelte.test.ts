import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    sessions: writable({ sessions: [] }),
    activeAgents: writable([
      { id: 'worker-1', role: 'Queen', config: { label: 'Alpha' } },
      { id: 'worker-2', role: 'Queen', config: { label: 'Beta' } },
    ]),
  };
});

import Harness from '../ui/DialogComposer.test.svelte';

const originalRects = Object.getOwnPropertyDescriptor(Range.prototype, 'getClientRects');
beforeEach(() => {
  Object.defineProperty(Range.prototype, 'getClientRects', {
    configurable: true, value: () => [{ left: 173, bottom: 281 }],
  });
});
afterEach(() => {
  cleanup(); document.body.innerHTML = '';
  if (originalRects) Object.defineProperty(Range.prototype, 'getClientRects', originalRects);
  else Reflect.deleteProperty(Range.prototype, 'getClientRects');
});

describe('composer caret menus', () => {
  it.each([['@', 'Mentions'], ['/', 'Commands']])('keeps composer focus while %s opens, navigates and selects inside a dialog', async (trigger, label) => {
    const view = render(Harness);
    await new Promise((resolve) => requestAnimationFrame(resolve));
    const editor = view.getByRole('textbox', { name: 'Write task' }); editor.focus();
    editor.textContent = trigger;
    const range = document.createRange(); range.setStart(editor.firstChild!, trigger.length); range.collapse(true);
    window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range);
    await fireEvent.input(editor);
    const menu = await view.findByRole('listbox', { name: label });
    const dialog = view.getByRole('dialog', { name: 'Composer dialog' });
    expect(dialog.contains(menu)).toBe(false);
    expect(menu.style.left).toBe('173px'); expect(menu.style.top).toBe('285px');
    expect(document.activeElement).toBe(editor);
    await fireEvent.keyDown(editor, { key: 'ArrowDown' });
    const options = Array.from(menu.querySelectorAll('[role="option"]'));
    expect(options.length).toBeGreaterThan(1);
    expect(options[1].getAttribute('aria-selected')).toBe('true');
    expect(options[0].getAttribute('aria-selected')).toBe('false');
    expect(document.activeElement).toBe(editor);
    options[1].dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, cancelable: true, button: 0, clientX: 100, clientY: 100 }));
    await fireEvent.mouseDown(options[1]);
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(view.getByRole('dialog', { name: 'Composer dialog' })).toBe(dialog);
    expect(document.activeElement).toBe(editor);
    expect(editor.querySelector('[data-token-value]')).toBeTruthy();
    await waitFor(() => expect(view.queryByRole('listbox')).toBeNull());
  });
});
