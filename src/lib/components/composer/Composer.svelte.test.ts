import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    sessions: writable({ sessions: [] }),
    activeAgents: writable([{ id: 'worker-1', role: 'Queen', config: { label: 'Alpha' } }]),
  };
});
vi.mock('$lib/composer/sources', async (original) => ({
  ...await original<typeof import('$lib/composer/sources')>(),
  listSessionFiles: vi.fn().mockResolvedValue([]),
}));

import Composer, { type ComposerControls } from './Composer.svelte';
import Toolbar from './ComposerToolbar.svelte';
import Harness from './Composer.test.svelte';
import { composerDraft } from '$lib/stores/composerDraft';
import { pendingContext, type PendingContext } from '$lib/stores/pendingContext';

const originalRects = Object.getOwnPropertyDescriptor(Range.prototype, 'getClientRects');
beforeEach(() => {
  Object.defineProperty(Range.prototype, 'getClientRects', {
    configurable: true, value: () => [{ left: 120, bottom: 240 }],
  });
});
afterEach(() => {
  cleanup();
  composerDraft.flush();
  pendingContext.clear();
  vi.restoreAllMocks();
  if (originalRects) Object.defineProperty(Range.prototype, 'getClientRects', originalRects);
  else Reflect.deleteProperty(Range.prototype, 'getClientRects');
});

async function write(editor: HTMLElement, text: string) {
  editor.focus();
  editor.textContent = text;
  const range = document.createRange();
  range.selectNodeContents(editor);
  range.collapse(false);
  window.getSelection()!.removeAllRanges();
  window.getSelection()!.addRange(range);
  await fireEvent.input(editor);
}

describe('composer card contract (G20)', () => {
  it('F5: serializes consume before any rerender and releases the guard after submission', async () => {
    let resolve!: (context: PendingContext | null) => void;
    const consume = vi.spyOn(pendingContext, 'consume').mockImplementationOnce(() => new Promise((done) => { resolve = done; })).mockResolvedValue(null);
    const onsubmit = vi.fn();
    const view = render(Harness, { sessionId: 's', onsubmit });
    const editor = view.getByRole('textbox');
    await write(editor, 'original text');
    editor.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    editor.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(consume, 'one consume before rerender').toHaveBeenCalledOnce();
    await waitFor(() => expect((view.getByRole('button', { name: 'Send message' }) as HTMLButtonElement).disabled, 'toolbar disabled during consume').toBe(true));
    resolve({ sessionId: 's', agentId: null, kind: 'selection', text: 'selected evidence', capturedAt: 0 });
    await waitFor(() => expect(onsubmit).toHaveBeenCalledOnce());
    expect(onsubmit).toHaveBeenCalledWith('[Operator context]\nselected evidence\n[/Operator context]\n\noriginal text');
    await write(editor, 'later text');
    await fireEvent.keyDown(editor, { key: 'Enter' });
    await waitFor(() => expect(onsubmit).toHaveBeenCalledTimes(2));
    expect(onsubmit).toHaveBeenLastCalledWith('later text');
  });

  it('F5: releases the guard after a rejected consume and an empty submission', async () => {
    const consume = vi.spyOn(pendingContext, 'consume').mockRejectedValueOnce(new Error('consume fixture')).mockResolvedValue(null);
    let controls!: ComposerControls;
    const onsubmit = vi.fn();
    const view = render(Harness, { sessionId: 's', onsubmit, observeControls: (current) => (controls = current) });
    const editor = view.getByRole('textbox');
    await write(editor, 'preserved draft');
    await expect(controls.submit()).rejects.toThrow('consume fixture');
    expect(editor.textContent).toBe('preserved draft');
    await waitFor(() => expect((view.getByRole('button', { name: 'Send message' }) as HTMLButtonElement).disabled, 'rejected consume releases toolbar').toBe(false));
    await write(editor, '');
    await controls.submit();
    expect(onsubmit).not.toHaveBeenCalled();
    await write(editor, 'retry after empty');
    await controls.submit();
    expect(consume).toHaveBeenCalledTimes(3);
    expect(onsubmit).toHaveBeenCalledOnce();
    expect(onsubmit).toHaveBeenCalledWith('retry after empty');
  });

  it('F4: reacts to same-session context and sends a context-only message by pointer', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response());
    const context: PendingContext = { sessionId: 's', agentId: null, kind: 'selection', text: 'selection only', capturedAt: 0 };
    const consume = vi.spyOn(pendingContext, 'consume').mockImplementation(async () => { pendingContext.clear(); return context; });
    const onsubmit = vi.fn();
    const view = render(Harness, { sessionId: 's', onsubmit });
    const send = view.getByRole('button', { name: 'Send message' }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    await pendingContext.capture({ ...context, sessionId: 'other' });
    await waitFor(() => expect(send.disabled).toBe(true));
    await pendingContext.capture(context);
    await waitFor(() => expect(send.disabled, 'same-session context enables pointer send').toBe(false));
    await fireEvent.click(send);
    await waitFor(() => expect(onsubmit).toHaveBeenCalledOnce());
    expect(onsubmit).toHaveBeenCalledWith(pendingContext.render(context));
    expect(consume).toHaveBeenCalledOnce();
    expect(consume).toHaveBeenCalledWith('s');
    await waitFor(() => expect(send.disabled).toBe(true));
    await pendingContext.capture(context);
    await waitFor(() => expect(send.disabled).toBe(false));
    await view.rerender({ disabled: true });
    expect(send.disabled).toBe(true);
    await view.rerender({ disabled: false, onsubmit: undefined });
    expect(send.disabled).toBe(true);
  });

  it('F4: Enter submits the same context-only payload', async () => {
    const context: PendingContext = { sessionId: 's', agentId: null, kind: 'selection', text: 'selection only', capturedAt: 0 };
    const consume = vi.spyOn(pendingContext, 'consume').mockResolvedValue(context);
    const onsubmit = vi.fn();
    const view = render(Harness, { sessionId: 's', onsubmit });
    await fireEvent.keyDown(view.getByRole('textbox'), { key: 'Enter' });
    await waitFor(() => expect(onsubmit).toHaveBeenCalledOnce());
    expect(onsubmit).toHaveBeenCalledWith(pendingContext.render(context));
    expect(consume).toHaveBeenCalledOnce();
    expect(consume).toHaveBeenCalledWith('s');
  });

  it('T15: closes the portalled add menu when sending begins without moving editor focus', async () => {
    const view = render(Harness, { onsubmit: vi.fn(), sending: false });
    const editor = view.getByRole('textbox'); editor.focus();
    await fireEvent.click(view.getByRole('button', { name: 'Add mention or command' }));
    expect(view.getByRole('menu', { name: 'Add to message' })).toBeTruthy();
    await view.rerender({ sending: true });
    expect(view.queryByRole('menu', { name: 'Add to message' }), 'sending must close the portalled toolbar menu').toBeNull();
    expect(document.activeElement).toBe(editor);
  });

  it('Enter submits once, Shift+Enter stays a newline, and no submit handler never clears text', async () => {
    const onsubmit = vi.fn();
    const view = render(Composer, { sessionId: null, persistDraft: false, onsubmit });
    const editor = view.getByRole('textbox');
    await write(editor, 'draft text');
    const shift = new KeyboardEvent('keydown', { key: 'Enter', shiftKey: true, bubbles: true, cancelable: true });
    editor.dispatchEvent(shift);
    expect(shift.defaultPrevented).toBe(false);
    expect(onsubmit).not.toHaveBeenCalled();
    await fireEvent.keyDown(editor, { key: 'Enter' });
    await waitFor(() => expect(onsubmit).toHaveBeenCalledOnce());
    expect(onsubmit).toHaveBeenCalledWith('draft text');
    expect(editor.textContent).toBe('');
    await view.rerender({ onsubmit: undefined });
    await write(editor, 'dialog text');
    const enter = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
    editor.dispatchEvent(enter);
    expect(enter.defaultPrevented).toBe(false);
    expect(editor.textContent).toBe('dialog text');
    expect(view.queryByRole('group', { name: 'Message controls' })).toBeNull();
  });

  it('toolbar actions preserve editor focus and never submit a surrounding form', async () => {
    const onsubmit = vi.fn();
    const onform = vi.fn();
    const view = render(Harness, { onsubmit, onform });
    const editor = view.getByRole('textbox');
    await write(editor, 'keep this');
    const add = view.getByRole('button', { name: 'Add mention or command' });
    const down = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    add.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true); // Proves focus retention without native jsdom mouse focus.
    await fireEvent.click(add);
    expect(document.activeElement).toBe(editor);
    const mention = view.getByRole('menuitem', { name: 'Mention' });
    const menuDown = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    mention.dispatchEvent(menuDown);
    expect(menuDown.defaultPrevented).toBe(true);
    await fireEvent.click(mention);
    expect(document.activeElement).toBe(editor);
    expect(onsubmit).not.toHaveBeenCalled();
    expect(onform).not.toHaveBeenCalled();
    expect(await view.findByRole('listbox', { name: 'Mentions' })).toBeTruthy();
    await fireEvent.keyDown(editor, { key: 'Enter' });
    expect(editor.querySelector('[data-token-value]')).toBeTruthy();
    expect(onsubmit).not.toHaveBeenCalled();
    const send = view.getByRole('button', { name: 'Send message' });
    await fireEvent.mouseDown(send);
    await fireEvent.click(send);
    await waitFor(() => expect(onsubmit).toHaveBeenCalledOnce());
    expect(onsubmit.mock.calls[0][0]).toContain('keep this');
    expect(onform).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(editor);
  });

  it('dismisses the add menu with Escape while pointer interaction keeps focus in the editor', async () => {
    const view = render(Harness);
    const editor = view.getByRole('textbox');
    await write(editor, 'draft');
    await fireEvent.click(view.getByRole('button', { name: 'Add mention or command' }));
    expect(view.getByRole('menu')).toBeTruthy();
    await fireEvent.keyDown(editor, { key: 'Escape' });
    expect(view.queryByRole('menu')).toBeNull();
    expect(document.activeElement).toBe(editor);
  });

  it('opens Command through the existing slash menu and flattens the selected token', async () => {
    const onsubmit = vi.fn();
    const view = render(Harness, { onsubmit });
    const editor = view.getByRole('textbox');
    await write(editor, 'Please');
    await fireEvent.click(view.getByRole('button', { name: 'Add mention or command' }));
    await fireEvent.click(view.getByRole('menuitem', { name: 'Command' }));
    expect(view.getByRole('listbox', { name: 'Commands' })).toBeTruthy();
    await fireEvent.keyDown(editor, { key: 'Enter' });
    const token = editor.querySelector<HTMLElement>('[data-token-value]');
    expect(token).toBeTruthy();
    await fireEvent.keyDown(editor, { key: 'Enter' });
    await waitFor(() => expect(onsubmit).toHaveBeenCalledOnce());
    expect(onsubmit.mock.calls[0][0]).toContain(token!.dataset.tokenValue);
  });

  it('focuses the editor from unused card space and puts editor before toolbar in tab order', async () => {
    const view = render(Harness, { onsubmit: vi.fn() });
    const editor = view.getByRole('textbox');
    const card = editor.parentElement!;
    await fireEvent.pointerDown(card);
    expect(document.activeElement).toBe(editor);
    const tabbables = Array.from(card.querySelectorAll<HTMLElement>('[tabindex="0"], button:not(:disabled)'));
    expect(tabbables[0]).toBe(editor);
    expect(tabbables[1]).toBe(view.getByRole('button', { name: 'Add mention or command' }));
    expect((view.getByRole('button', { name: 'Send message' }) as HTMLButtonElement).disabled).toBe(true);
    expect(view.queryByRole('button', { name: /mic|upload|stop/i })).toBeNull();
  });

  it('supports keyboard navigation of the add menu and dismisses it with Escape', async () => {
    const view = render(Harness);
    const add = view.getByRole('button', { name: 'Add mention or command' });
    add.focus();
    await fireEvent.keyDown(add, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(view.getByRole('menuitem', { name: 'Mention' }));
    await fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(view.getByRole('menuitem', { name: 'Command' }));
    await fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    expect(view.queryByRole('menu')).toBeNull();
    expect(document.activeElement).toBe(add);
  });

  it('portals both toolbar and caret menus outside dialogs without moving editor focus', async () => {
    const view = render(Harness, { dialog: true });
    await new Promise((resolve) => requestAnimationFrame(resolve));
    const dialog = view.getByRole('dialog', { name: 'Toolbar dialog' });
    const editor = view.getByRole('textbox');
    await write(editor, '');
    await fireEvent.click(view.getByRole('button', { name: 'Add mention or command' }));
    const menu = view.getByRole('menu', { name: 'Add to message' });
    expect(dialog.contains(menu)).toBe(false);
    await fireEvent.pointerDown(view.getByRole('menuitem', { name: 'Mention' }));
    await fireEvent.mouseDown(view.getByRole('menuitem', { name: 'Mention' }));
    await fireEvent.click(view.getByRole('menuitem', { name: 'Mention' }));
    const mentions = await view.findByRole('listbox', { name: 'Mentions' });
    expect(dialog.contains(mentions)).toBe(false);
    expect(document.activeElement).toBe(editor);
    expect(view.getByRole('dialog', { name: 'Toolbar dialog' })).toBe(dialog);
  });

  it('keeps drafts through remount, then clears only on submit', async () => {
    const sessionId = 'composer-card-draft';
    composerDraft.load(sessionId);
    composerDraft.update('saved fixture');
    composerDraft.flush();
    const onsubmit = vi.fn();
    const view = render(Harness, { onsubmit, sessionId, persistDraft: true });
    const editor = view.getByRole('textbox');
    expect(editor.textContent).toBe('saved fixture');
    await write(editor, 'changed fixture');
    expect(composerDraft.read(sessionId)).toBe('changed fixture');
    view.unmount();
    const next = render(Harness, { onsubmit, sessionId, persistDraft: true });
    expect(next.getByRole('textbox').textContent).toBe('changed fixture');
    await fireEvent.keyDown(next.getByRole('textbox'), { key: 'Enter' });
    await waitFor(() => expect(onsubmit).toHaveBeenCalledOnce());
    expect(onsubmit).toHaveBeenCalledWith('changed fixture');
    expect(composerDraft.read(sessionId)).toBe('');
  });

  it('omits unknown toolbar values and disables sending while a message is in flight', async () => {
    const controls = { canSend: true, submit: vi.fn(), openMenu: vi.fn() };
    const view = render(Toolbar, { controls, sending: true });
    expect(view.queryByText('Full access')).toBeNull();
    expect(view.container.querySelector('.model-facts')!.textContent?.trim()).toBe('');
    const send = view.getByRole('button', { name: 'Sending message' }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    await fireEvent.click(send);
    expect(controls.submit).not.toHaveBeenCalled();
    expect(view.queryByRole('combobox')).toBeNull();
  });
});
