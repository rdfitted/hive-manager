// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, within } from '@testing-library/svelte';
import { get } from 'svelte/store';

vi.mock('./StatusPanel.svelte', () => ({ default: () => {} }));
vi.mock('./PlanView.svelte', () => ({ default: () => {} }));
vi.mock('./workgraph/WorkGraphView.svelte', () => ({ default: () => {} }));
vi.mock('./CoordinationPanel.svelte', () => ({ default: () => {} }));
vi.mock('./ConversationViewer.svelte', () => ({ default: () => {} }));
vi.mock('./timeline/TimelineView.svelte', () => ({ default: () => {} }));
vi.mock('./SessionFilesView.svelte', () => ({ default: () => {} }));
vi.mock('./review/ReviewView.svelte', () => ({ default: () => {} }));
vi.mock('./replay/ReplayView.svelte', () => ({ default: () => {} }));
vi.mock('./replay/ReplayControls.svelte', () => ({ default: () => {} }));
vi.mock('./artifacts/ArtifactBrowser.svelte', () => ({ default: () => {} }));
vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return { activeSession: writable({ id: 'session-a', project_path: 'C:/project', agents: [] }) };
});

import RightPanel from './RightPanel.svelte';
import TitleBar from './shell/TitleBar.svelte';
import { layout } from '$lib/stores/layout';

beforeEach(() => {
  layout.setRightTab('overview');
  if (get(layout).rightCollapsed) layout.toggleRight();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('RightPanel header tabs and title-bar toggle', () => {
  it.each(['close', 'active tab'])('returns focus after collapsing with the %s control', async (action) => {
    const panel = render(RightPanel);
    const toolbar = render(TitleBar);
    const toggle = toolbar.container.querySelector<HTMLButtonElement>('#title-bar-panel-toggle')!;
    const focus = vi.spyOn(toggle, 'focus');
    const control = within(panel.container).getByRole('button', { name: action === 'close' ? /Collapse panel/ : 'Overview' });
    control.focus();
    await fireEvent.click(control);
    expect(get(layout).rightCollapsed).toBe(true);
    expect(document.activeElement).toBe(toggle);
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
  });

  it('keeps focus on an inactive tab when switching views', async () => {
    const panel = render(RightPanel);
    render(TitleBar);
    const activity = panel.getByRole('button', { name: 'Activity' });
    activity.focus();
    await fireEvent.click(activity);
    expect(get(layout).rightCollapsed).toBe(false);
    expect(document.activeElement).toBe(activity);
  });

  it('does not steal terminal focus on an external panel collapse', async () => {
    render(RightPanel);
    render(TitleBar);
    const terminal = document.createElement('textarea');
    document.body.append(terminal);
    terminal.focus();
    layout.toggleRight();
    await import('svelte').then(({ tick }) => tick());
    expect(document.activeElement).toBe(terminal);
    terminal.remove();
  });

  it('exposes exactly three header tabs and preserves activate-to-collapse behavior', async () => {
    const view = render(RightPanel);
    const toolbar = render(TitleBar);
    const header = view.container.querySelector('.panel-header')!;
    expect(view.container.querySelector('.rail')).toBeNull();
    expect([...header.querySelectorAll('button[aria-pressed]')].map((button) => button.getAttribute('aria-label')))
      .toEqual(['Overview', 'Activity', 'Changes']);
    const activity = view.getByRole('button', { name: 'Activity' });
    await fireEvent.click(activity);
    expect(get(layout)).toMatchObject({ rightTab: 'activity', rightCollapsed: false });
    expect(view.getByRole('heading', { name: 'Activity' })).toBeTruthy();
    await fireEvent.click(activity);
    expect(get(layout).rightCollapsed).toBe(true);
    expect((view.container.querySelector('.panel-drawer') as HTMLElement).inert).toBe(true);
    expect(view.container.querySelector('.panel-drawer')?.getAttribute('aria-hidden')).toBe('true');
    expect((view.container.querySelector('.right-panel') as HTMLElement).style.width).toBe('0px');
    await fireEvent.click(within(toolbar.container).getByRole('button', { name: /Expand panel/ }));
    await fireEvent.click(view.getByRole('button', { name: 'Changes' }));
    expect(get(layout)).toMatchObject({ rightTab: 'changes', rightCollapsed: false });
    expect(view.getByRole('heading', { name: 'Changes' })).toBeTruthy();
  });

  it('has a visible button that collapses and expands the panel', async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32');
    const panel = render(RightPanel);
    const view = render(TitleBar);
    const toggle = within(view.container).getByRole('button', { name: 'Collapse panel (Ctrl+J)' });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(toggle.querySelector('svg')).toBeTruthy();

    await fireEvent.click(toggle);
    expect(get(layout).rightCollapsed).toBe(true);
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(toggle.getAttribute('aria-label')).toBe('Expand panel (Ctrl+J)');
    expect((panel.container.querySelector('.right-panel') as HTMLElement).style.width).toBe('0px');

    await fireEvent.click(toggle);
    expect(get(layout).rightCollapsed).toBe(false);
    expect((panel.container.querySelector('.right-panel') as HTMLElement).style.width).toBe(`${get(layout).rightWidth}px`);
  });

  it('labels the shortcut with ⌘ on macOS', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel');
    const view = render(TitleBar);
    expect(view.getByRole('button', { name: 'Collapse panel (⌘J)' })).toBeTruthy();
  });
});
