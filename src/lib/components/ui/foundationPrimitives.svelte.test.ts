import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { PlusIcon } from 'phosphor-svelte';
import { createRawSnippet, type ComponentProps } from 'svelte';
import { afterEach, describe, expect, expectTypeOf, it, vi } from 'vitest';
import { HiveMark, IconButton } from './index';
import TooltipTestHarness from './TooltipTestHarness.test.svelte';

afterEach(cleanup);

describe('IconButton', () => {
  it('requires an accessible name and tooltip, and describes the focused button', async () => {
    expectTypeOf<Pick<ComponentProps<typeof IconButton>, 'label' | 'tooltip'>>()
      .toEqualTypeOf<{ label: string; tooltip: string }>();

    const onclick = vi.fn();
    render(IconButton, {
      label: 'New worker', tooltip: 'Add a worker', shortcut: 'Ctrl+N',
      icon: PlusIcon, pressed: false, onclick, id: 'add-worker',
      class: 'toolbar-button', size: 'sm', 'aria-describedby': 'worker-help',
    });
    const button = screen.getByRole('button', { name: 'New worker' });
    expect(button.id).toBe('add-worker');
    expect(button.getAttribute('aria-pressed')).toBe('false');
    expect(button.classList.contains('toolbar-button')).toBe(true);
    expect(button.querySelector('[aria-hidden="true"] svg')?.getAttribute('width')).toBe('14');
    expect(button.getAttribute('aria-describedby')).toBe('worker-help');

    button.focus();
    const tooltip = await screen.findByRole('tooltip');
    expect(tooltip.textContent).toContain('Add a worker');
    expect(tooltip.querySelector('kbd')?.textContent).toBe('Ctrl+N');
    await waitFor(() => expect(button.getAttribute('aria-describedby')).toBe(`worker-help ${tooltip.id}`));
    await fireEvent.click(button);
    expect(onclick).toHaveBeenCalledOnce();
    await waitFor(() => expect(screen.queryByRole('tooltip')).toBeNull());
  });

  it('renders a snippet icon and respects native disabled behavior', async () => {
    const onclick = vi.fn();
    const icon = createRawSnippet(() => ({ render: () => '<span>+</span>' }));
    render(IconButton, {
      label: 'Add', tooltip: 'Add item', icon, variant: 'primary', disabled: true, onclick,
    });
    const button = screen.getByRole<HTMLButtonElement>('button', { name: 'Add' });
    expect(button.disabled).toBe(true);
    expect(button.dataset.variant).toBe('primary');
    expect(button.querySelector('[aria-hidden="true"]')?.textContent?.trim()).toBe('+');
    button.click();
    await fireEvent.focus(button);
    expect(onclick).not.toHaveBeenCalled();
    expect(screen.queryByRole('tooltip')).toBeNull();
  });
});

describe('Tooltip', () => {
  it.each([false, true])('provides one focus tooltip (shared provider: %s)', async (provider) => {
    render(TooltipTestHarness, { provider });
    const trigger = screen.getByRole('button', { name: 'Help' });
    expect(trigger.getAttribute('data-delay-duration')).toBe(provider ? '42' : '700');
    expect(screen.queryByRole('tooltip')).toBeNull();

    trigger.focus();
    const tooltip = await screen.findByRole('tooltip');
    expect(tooltip.classList.contains('lattice-tooltip')).toBe(true);
    expect(tooltip.querySelector('kbd')?.textContent).toBe('F1');

    await fireEvent.keyDown(trigger, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('tooltip')).toBeNull());
  });
});

describe('HiveMark', () => {
  it('has an accessible title and a single cut-out path that inherits the accent color', () => {
    render(HiveMark, { size: 20, title: 'Hive Manager' });
    const mark = screen.getByRole('img', { name: 'Hive Manager' });
    expect(mark.getAttribute('width')).toBe('20');
    expect(mark.getAttribute('height')).toBe('20');
    const paths = mark.querySelectorAll('path');
    expect(paths.length).toBe(1);
    expect(paths[0].getAttribute('fill-rule')).toBe('evenodd');
    expect(paths[0].getAttribute('fill')).toBe('currentColor');
  });
});
