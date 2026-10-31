import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get } from 'svelte/store';
// @ts-expect-error Node types are intentionally absent from the frontend tsconfig.
import { readFileSync } from 'node:fs';

const native = vi.hoisted(() => ({ minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn() }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => native }));
vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return { activeSession: writable({
    id: 'session-a', project_path: 'C:/projects/example', agents: [
      { id: 'queen', config: { label: 'Queen' } },
    ],
  }) };
});

import TitleBar from './TitleBar.svelte';
import TitleBarDialog from './TitleBarDialog.test.svelte';
import { layout } from '$lib/stores/layout';
import { ui } from '$lib/stores/ui';

beforeEach(() => {
  vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Windows NT 10.0');
  vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32');
  for (const action of Object.values(native)) action.mockReset().mockResolvedValue(undefined);
  layout.setLeftCollapsed(false);
  layout.setRightTab('overview');
  layout.setSessionViewMode('focus');
  ui.setFocusedAgent('queen');
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('TitleBar', () => {
  it('shows the breadcrumb and keyboard-reachable controls, and calls the exact native operations', async () => {
    const view = render(TitleBar);
    expect(view.getByText('example')).toBeTruthy();
    expect(view.getByText('Queen')).toBeTruthy();
    for (const [label, action] of [
      ['Minimize window', 'minimize'], ['Maximize or restore window', 'toggleMaximize'], ['Close window', 'close'],
    ] as const) {
      const button = view.getByRole('button', { name: label });
      expect(button.tabIndex).toBe(0);
      button.focus();
      expect(document.activeElement).toBe(button);
      await fireEvent.click(button);
      expect(native[action]).toHaveBeenCalledOnce();
    }
    for (const button of view.container.querySelectorAll('button')) expect(button.tabIndex).toBe(0);
  });

  it('keeps drag targets on passive surfaces and off every interactive descendant', () => {
    const view = render(TitleBar);
    const bar = view.getByRole('banner');
    for (const target of [bar, ...bar.querySelectorAll('.breadcrumb, .breadcrumb span, .window-controls')]) {
      expect(target.hasAttribute('data-tauri-drag-region')).toBe(true);
      expect(target.hasAttribute('data-window-chrome')).toBe(true);
    }
    for (const button of bar.querySelectorAll('button')) {
      expect(button.hasAttribute('data-tauri-drag-region')).toBe(false);
      expect(button.querySelector('[data-tauri-drag-region]')).toBeNull();
    }
    const source = readFileSync('src/lib/components/shell/TitleBar.svelte', 'utf8');
    expect(source).toMatch(/\.shell-actions\s*\{[^}]*pointer-events:\s*none/s);
    expect(source).toMatch(/\.window-control :global\(svg\)\s*\{\s*pointer-events:\s*none/s);
  });

  it('keeps native chrome live above the Bits overlay while application controls are inert', async () => {
    const view = render(TitleBarDialog);
    await fireEvent.click(view.getByRole('button', { name: 'Open test dialog' }));
    await waitFor(() => expect(document.body.querySelector('[aria-modal="true"]')).toBeTruthy());
    await tick();
    for (const group of view.container.querySelectorAll<HTMLElement>('.shell-actions')) expect(group.inert).toBe(true);
    expect(document.body.style.pointerEvents).toBe('none');
    const close = within(view.container).getByRole('button', { name: 'Close window' });
    expect(close.closest('[inert]')).toBeNull();
    expect(close.hasAttribute('data-window-chrome')).toBe(true);
    await fireEvent.pointerDown(close);
    await fireEvent.click(close);
    expect(native.close).toHaveBeenCalledOnce();
    expect(document.body.querySelector('[aria-modal="true"]')).toBeTruthy();
    const source = readFileSync('src/lib/components/shell/TitleBar.svelte', 'utf8');
    const dialog = readFileSync('src/lib/components/ui/Dialog.svelte', 'utf8');
    const overlayBase = Number(dialog.match(/z-index:\s*calc\((\d+)\s*\+/)![1]);
    const chromeLayer = Number(source.match(/\[data-window-chrome\]\s*\{[^}]*z-index:\s*(\d+)/s)![1]);
    expect(chromeLayer).toBeGreaterThan(overlayBase + 1);
    expect(source).toMatch(/\[data-window-chrome\]\s*\{[^}]*pointer-events:\s*auto/s);
    await fireEvent.click(within(document.body).getByRole('button', { name: 'Close test dialog' }));
    await waitFor(() => expect(view.container.querySelector('[inert]')).toBeNull());
  });

  it('collapses and expands the sidebar and dispatches the palette shortcut', async () => {
    const view = render(TitleBar);
    const toggle = view.getByRole('button', { name: 'Collapse sidebar (Ctrl+B)' });
    await fireEvent.click(toggle);
    expect(get(layout).leftCollapsed).toBe(true);
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    await fireEvent.click(toggle);
    expect(get(layout).leftCollapsed).toBe(false);
    const listener = vi.fn();
    window.addEventListener('keydown', listener);
    await fireEvent.click(view.getByRole('button', { name: /Open command palette/ }));
    expect(listener).toHaveBeenCalledWith(expect.objectContaining({ key: 'P', ctrlKey: true, shiftKey: true }));
    window.removeEventListener('keydown', listener);
  });

  it.each([false, true])('preserves intended focus after a dialog closes (another control focused: %s)', async (anotherControlFocused) => {
    const view = render(TitleBarDialog);
    const trigger = view.getByRole('button', { name: /Open command palette/ });
    const controls = trigger.closest<HTMLElement>('.shell-actions')!;
    const other = document.createElement('textarea');
    other.setAttribute('aria-label', 'Existing terminal focus');
    view.container.append(other);
    trigger.focus();
    const focus = trigger.focus.bind(trigger);
    // jsdom does not consistently implement the browser rule that inert rejects focus.
    vi.spyOn(trigger, 'focus').mockImplementation((options) => {
      if (!controls.inert) focus(options);
      // Simulate a terminal or other control receiving explicit focus during teardown.
      else if (anotherControlFocused) other.focus();
    });
    await fireEvent.click(trigger);
    await waitFor(() => expect(controls.inert).toBe(true));
    const close = within(document.body).getByRole('button', { name: 'Close test dialog' });
    close.focus();
    await fireEvent.click(close);
    await waitFor(() => {
      expect(controls.inert).toBe(false);
      expect(document.activeElement).toBe(anotherControlFocused ? other : trigger);
    });
  });

  it.each(['P', 'k', '/'])('returns keyboard focus to an application control after the %s dialog shortcut', async (key) => {
    const view = render(TitleBarDialog);
    const trigger = view.getByRole('button', { name: 'Collapse sidebar (Ctrl+B)' });
    const controls = trigger.closest<HTMLElement>('.shell-actions')!;
    trigger.focus();
    const focus = trigger.focus.bind(trigger);
    vi.spyOn(trigger, 'focus').mockImplementation((options) => {
      if (!controls.inert) focus(options);
    });
    await fireEvent.keyDown(window, { key, ctrlKey: true, shiftKey: key === 'P' });
    await waitFor(() => expect(controls.inert).toBe(true));
    const close = within(document.body).getByRole('button', { name: 'Close test dialog' });
    close.focus();
    await fireEvent.click(close);
    await waitFor(() => expect(document.activeElement).toBe(trigger));
  });

  it('reserves macOS traffic lights and leaves Linux controls to the native frame', () => {
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Macintosh; Intel Mac OS X');
    const mac = render(TitleBar);
    expect(mac.container.querySelector('.traffic-light-space')).toBeTruthy();
    expect(mac.queryByRole('button', { name: 'Close window' })).toBeNull();
    cleanup();
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('X11; Linux x86_64');
    const linux = render(TitleBar);
    expect(linux.container.querySelector('.traffic-light-space')).toBeNull();
    expect(linux.queryByRole('button', { name: 'Close window' })).toBeNull();
  });
});
