import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import ResourceStrip from './ResourceStrip.svelte';
afterEach(cleanup);
it('G59 resource tiles show real counts/phase and toggle highlights', async () => {
  const onfilter = vi.fn();
  const view = render(ResourceStrip, { attention: 2, waiting: 3, stale: 1, phase: 'QaInProgress', filter: null, onfilter });
  expect(view.getByText('QaInProgress')).toBeTruthy();
  expect(view.getByRole('button', { name: 'Attention 2' })).toBeTruthy();
  expect(view.getByRole('button', { name: 'Waiting 3' })).toBeTruthy();
  const tile = view.getByRole('button', { name: 'Stale 1' });
  await fireEvent.click(tile); expect(onfilter).toHaveBeenLastCalledWith('stale');
  await view.rerender({ filter: 'stale' });
  expect(tile.getAttribute('aria-pressed')).toBe('true');
  await fireEvent.click(tile); expect(onfilter).toHaveBeenLastCalledWith(null);
});
