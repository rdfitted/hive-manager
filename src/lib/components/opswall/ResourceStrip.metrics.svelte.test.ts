import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import ResourceStrip from './ResourceStrip.svelte';

afterEach(cleanup);

it('exposes five metric labels as text with compact values and an explicit unavailable state', () => {
  const view = render(ResourceStrip, {
    attention: 2, waiting: 3, stale: 1, phase: 'Running', filter: null, onfilter: vi.fn(),
    metrics: { agents: 7, running: 4, elapsed: '3m', nodes: '2/5', files: null },
  });
  const metrics = [...view.container.querySelectorAll('.metric')];
  expect(metrics.map(metric => metric.textContent?.trim().replace(/\s+/g, ' '))).toEqual([
    'Agents 7', 'Running 4', 'Elapsed 3m', 'Nodes 2/5', 'Files unavailable —',
  ]);
  expect(metrics[4].querySelector('[aria-hidden="true"]:last-child')?.textContent).toBe('—');
  for (const name of ['Attention 2', 'Waiting 3', 'Stale 1']) {
    expect(view.getByRole('button', { name })).toBeTruthy();
  }
});
