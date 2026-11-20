// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';

const mounts = vi.hoisted(() => ({ status: vi.fn(), plan: vi.fn(), graph: vi.fn() }));
vi.mock('../StatusPanel.svelte', () => ({ default: mounts.status }));
vi.mock('../PlanView.svelte', () => ({ default: mounts.plan }));
vi.mock('../workgraph/WorkGraphView.svelte', () => ({ default: mounts.graph }));
import OverviewTab from './OverviewTab.svelte';
afterEach(() => { cleanup(); vi.clearAllMocks(); });

it('renders Status, Plan and Graph from the single inspector selection', async () => {
  const view = render(OverviewTab);
  expect(mounts.status).toHaveBeenCalled();
  await view.rerender({ tab: 'plan' });
  expect(mounts.plan).toHaveBeenCalled();
  await view.rerender({ tab: 'graph' });
  expect(mounts.graph).toHaveBeenCalled();
  expect(view.queryByRole('group', { name: 'Overview sections' })).toBeNull();
  expect(view.queryByRole('tablist')).toBeNull();
});
