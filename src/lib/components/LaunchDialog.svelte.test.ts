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
  { provider: 'codex', tier: 'low', model: 'gpt-6-luna', flags: ['-c', 'model_reasoning_effort="medium"'] },
  { provider: 'codex', tier: 'medium', model: 'gpt-6-sol', flags: ['-c', 'model_reasoning_effort="medium"'] },
  { provider: 'codex', tier: 'high', model: 'gpt-6-sol', flags: ['-c', 'model_reasoning_effort="xhigh"'] },
  { provider: 'codex', tier: 'critical', model: 'gpt-6-astra', flags: ['-c', 'model_reasoning_effort="max"'] },
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
    const codexLadder = view.getByLabelText('Codex tier ladder').textContent;
    expect(codexLadder).toContain('gpt-6-luna');
    expect(codexLadder).toContain('gpt-6-sol');
    expect(codexLadder).toContain('gpt-6-astra');
    expect(codexLadder).toContain('model_reasoning_effort="xhigh"');
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

describe('LaunchDialog QA defaults', () => {
  it('launches a Hive with QA enabled by default', async () => {
    const launchHive = vi.fn();
    const view = render(LaunchDialog, {
      props: { show: true },
      events: { launchHive },
    });
    await fireEvent.click(view.getByRole('button', { name: 'Browse' }));
    const qaCheckbox = view.getByRole('checkbox', { name: /QA: Evaluator \+ QA workers/ }) as HTMLInputElement;
    expect(qaCheckbox.checked).toBe(true);
    expect(view.getByText(/UI and A11Y workers need a rendered browser/)).toBeTruthy();
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));

    expect(launchHive).toHaveBeenCalledTimes(1);
    expect(launchHive.mock.calls[0]?.[0].detail).toEqual(expect.objectContaining({
      with_evaluator: true,
      evaluator_config: expect.objectContaining({ cli: expect.any(String) }),
      qa_workers: expect.arrayContaining([expect.objectContaining({ specialization: 'api' })]),
    }));
  });

  it('launches Solo with QA enabled by default', async () => {
    const launchSolo = vi.fn();
    const view = render(LaunchDialog, {
      props: { show: true },
      events: { launchSolo },
    });
    await fireEvent.click(view.getByRole('button', { name: 'Solo' }));
    await fireEvent.click(view.getByRole('button', { name: 'Browse' }));
    expect((view.getByRole('checkbox', { name: /QA: Evaluator \+ QA workers/ }) as HTMLInputElement).checked).toBe(true);
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));

    expect(launchSolo).toHaveBeenCalledTimes(1);
    expect(launchSolo.mock.calls[0]?.[0].detail).toEqual(expect.objectContaining({
      with_evaluator: true,
      evaluator_config: expect.objectContaining({ cli: expect.any(String) }),
      qa_workers: expect.arrayContaining([expect.objectContaining({ specialization: 'api' })]),
    }));
  });
});
