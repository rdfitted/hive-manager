import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';

const testMocks = vi.hoisted(() => ({
  open: vi.fn(),
  fetchCliHealth: vi.fn().mockResolvedValue({}),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: testMocks.open,
}));

vi.mock('./AgentConfigEditor.svelte', () => ({
  default: () => {},
  fetchCliHealth: testMocks.fetchCliHealth,
}));

vi.mock('./composer/Composer.svelte', () => ({ default: () => {} }));
vi.mock('./templates/TemplatePicker.svelte', () => ({ default: () => {} }));

import LaunchDialog from './LaunchDialog.svelte';

const ladderCells = [
  { provider: 'claude', tier: 'low', model: 'haiku', flags: [] },
  { provider: 'claude', tier: 'medium', model: 'sonnet', flags: [] },
  { provider: 'claude', tier: 'high', model: 'opus', flags: [] },
  { provider: 'claude', tier: 'critical', model: 'opus', flags: ['--settings', '{"effortLevel":"max"}'] },
  { provider: 'codex', tier: 'low', model: 'gpt-5.6-terra', flags: [] },
  { provider: 'codex', tier: 'medium', model: 'gpt-5.6-sol', flags: [] },
  { provider: 'codex', tier: 'high', model: 'gpt-5.6-sol', flags: ['-c', 'model_reasoning_effort="high"'] },
  { provider: 'codex', tier: 'critical', model: 'gpt-5.6-sol', flags: ['-c', 'model_reasoning_effort="ultra"'] },
];

function jsonResponse(payload: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: vi.fn().mockResolvedValue(payload),
  } as unknown as Response;
}

async function renderTierRoutingDialog() {
  const launchHive = vi.fn();
  const view = render(LaunchDialog, {
    props: { show: true },
    events: { launchHive },
  });

  await fireEvent.click(view.getByRole('button', { name: 'Browse' }));
  await waitFor(() => {
    expect((view.getByLabelText('Project Path') as HTMLInputElement).value).toBe('C:/code/project');
  });
  await fireEvent.click(view.getByRole('checkbox', {
    name: /Enable tier-based model routing/,
  }));

  return { ...view, launchHive };
}

beforeEach(() => {
  testMocks.open.mockReset().mockResolvedValue('C:/code/project');
  testMocks.fetchCliHealth.mockClear();
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({
    cells: ladderCells,
    omissions: [],
  })));
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  document.body.innerHTML = '';
});

describe('LaunchDialog tier routing', () => {
  it('renders distinct Claude and Codex ladder previews and emits the selected policy', async () => {
    const view = await renderTierRoutingDialog();

    await waitFor(() => {
      expect(view.getByRole('heading', { name: 'Claude ladder' })).toBeTruthy();
      expect(view.getByRole('heading', { name: 'Codex ladder' })).toBeTruthy();
    });
    expect(view.getByLabelText('Claude tier ladder').textContent).toContain('haiku');
    expect(view.getByLabelText('Codex tier ladder').textContent).toContain('gpt-5.6-sol');
    expect(fetch).toHaveBeenCalledWith(expect.stringMatching(
      /\/api\/tier-ladder\?project_path=C%3A%2Fcode%2Fproject$/,
    ));

    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));

    expect(view.launchHive).toHaveBeenCalledTimes(1);
    expect(view.launchHive.mock.calls[0]?.[0].detail.execution_policy.tier_policy).toEqual({
      enabled: true,
      ceiling_percent: 34,
      review_floor: 'high',
      ladder: {},
    });
  });

  it.each([0, 101, 1.5])(
    'rejects an invalid ceiling of %s before dispatch',
    async (ceiling) => {
      const view = await renderTierRoutingDialog();
      const ceilingInput = view.getByLabelText('Routing ceiling (%)') as HTMLInputElement;
      await fireEvent.input(ceilingInput, { target: { value: String(ceiling) } });

      await fireEvent.click(view.getByRole('button', { name: 'Launch' }));

      expect(view.getByRole('alert').textContent).toContain(
        'Tier-routing ceiling must be a whole number from 1 to 100.',
      );
      expect(view.launchHive).not.toHaveBeenCalled();
    },
  );

  it('restores a valid ceiling before routing is disabled and dispatched', async () => {
    const view = await renderTierRoutingDialog();
    const ceilingInput = view.getByLabelText('Routing ceiling (%)') as HTMLInputElement;
    await fireEvent.input(ceilingInput, { target: { value: '101' } });
    await fireEvent.click(view.getByRole('checkbox', {
      name: /Enable tier-based model routing/,
    }));
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));

    expect(view.launchHive).toHaveBeenCalledTimes(1);
    expect(view.launchHive.mock.calls[0]?.[0].detail.execution_policy.tier_policy).toEqual({
      enabled: false,
      ceiling_percent: 34,
      review_floor: 'high',
      ladder: {},
    });
  });
});
