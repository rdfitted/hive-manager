// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';

const mounts = vi.hoisted(() => ({ status: vi.fn(), plan: vi.fn(), graph: vi.fn() }));
vi.mock('../StatusPanel.svelte', () => ({ default: mounts.status }));
vi.mock('../PlanView.svelte', () => ({ default: mounts.plan }));
vi.mock('../workgraph/WorkGraphView.svelte', () => ({ default: mounts.graph }));
import OverviewTab from './OverviewTab.svelte';
afterEach(() => { cleanup(); vi.clearAllMocks(); });

it('composes Status, Plan and Graph through section controls', async () => {
  const view = render(OverviewTab);
  expect(mounts.status).toHaveBeenCalled();
  await fireEvent.click(view.getByRole('button', { name: 'Plan' }));
  expect(mounts.plan).toHaveBeenCalled();
  await fireEvent.click(view.getByRole('button', { name: 'Graph' }));
  expect(mounts.graph).toHaveBeenCalled();
  expect(view.queryByRole('tablist')).toBeNull();
});
