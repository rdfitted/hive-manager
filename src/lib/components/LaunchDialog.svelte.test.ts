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
  { provider: 'codex', tier: 'medium', model: 'gpt-6.1-sol', flags: ['-c', 'model_reasoning_effort="medium"'] },
  { provider: 'codex', tier: 'high', model: 'gpt-6.1-sol', flags: ['-c', 'model_reasoning_effort="xhigh"'] },
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

  await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));

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

describe('LaunchDialog modal presentation', () => {
  it('keeps a launch in progress open when Escape is pressed', async () => {
    const close = vi.fn();
    const view = render(LaunchDialog, { props: { show: true, launching: true }, events: { close } });
    const dialog = view.getByRole('dialog');
    await fireEvent.keyDown(dialog, { key: 'Escape', code: 'Escape' });
    expect(view.getByRole('dialog')).toBe(dialog);
    expect(close).not.toHaveBeenCalled();
    await view.rerender({ show: true, launching: false });
    await fireEvent.keyDown(dialog, { key: 'Escape', code: 'Escape' });
    await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
  });

  it('uses the shared modal primitive while inline presentation stays plain', async () => {
    const view = render(LaunchDialog, { props: { show: true } });
    expect(view.getByRole('dialog').getAttribute('aria-modal')).toBe('true');
    // The modal deliberately stays in the render container for frozen parity queries.
    expect(view.container.querySelector('[role="dialog"]')).toBeTruthy();
    await view.rerender({ show: true, presentation: 'inline' });
    expect(view.queryByRole('dialog')).toBeNull();
    expect(view.getByRole('region', { name: 'Hive Manager' })).toBeTruthy();
  });

  it('keeps Advanced Cancel available and dispatches exactly one close event', async () => {
    const close = vi.fn();
    const view = render(LaunchDialog, {
      props: { show: true, presentation: 'modal' },
      events: { close },
    });

    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));

    const cancel = view.getByRole('button', { name: 'Cancel' });
    expect(cancel).toBeTruthy();
    expect(close).not.toHaveBeenCalled();

    await fireEvent.click(cancel);

    expect(close).toHaveBeenCalledTimes(1);
  });
});

describe('LaunchDialog shared state', () => {
  it.each(['hive', 'research', 'fusion', 'debate', 'solo'] as const)(
    'retains %s composer edits through Advanced and back in the launched payload',
    async (mode) => {
      const launch = vi.fn();
      const view = render(LaunchDialog, {
        props: { show: true, initialMode: mode },
        events: { launchHive: launch, launchResearch: launch, launchFusion: launch, launchDebate: launch, launchSolo: launch },
      });
      await fireEvent.click(view.getByRole('button', { name: /Project/ }));
      await fireEvent.input(view.getByLabelText('What would you like to work on?'), {
        target: { value: 'Keep this task through every view' },
      });
      if (mode !== 'solo') {
        await fireEvent.change(view.getByLabelText('Workers'), { target: { value: '3' } });
      }
      await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
      await fireEvent.input(view.getByLabelText('Session Name (optional)'), { target: { value: 'Round trip' } });
      if (mode === 'solo') {
        expect((view.getByLabelText('Task Description') as HTMLTextAreaElement).value)
          .toBe('Keep this task through every view');
      } else if (mode === 'hive') {
        expect(view.getByRole('heading', { name: 'Coding Principals (3)' })).toBeTruthy();
      } else if (mode === 'research') {
        expect(view.getByRole('heading', { name: 'Researcher Roster (3)' })).toBeTruthy();
      } else {
        expect((view.getByLabelText(mode === 'fusion' ? 'Number of Variants' : 'Number of Debaters') as HTMLSelectElement).value).toBe('3');
      }
      await fireEvent.click(view.getByRole('button', { name: 'Back to composer' }));
      expect((view.getByLabelText('What would you like to work on?') as HTMLTextAreaElement).value)
        .toBe('Keep this task through every view');
      expect((view.getByLabelText('Workers') as HTMLSelectElement).value).toBe(mode === 'solo' ? '1' : '3');
      await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
      expect((view.getByLabelText('Session Name (optional)') as HTMLInputElement).value).toBe('Round trip');
      await fireEvent.click(view.getByRole('button', { name: 'Back to composer' }));
      await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
      expect(launch).toHaveBeenCalledTimes(1);
      const payload = launch.mock.calls[0][0].detail;
      expect(payload.name).toBe('Round trip');
      expect(mode === 'solo' ? payload.taskDescription : mode === 'fusion' ? payload.task_description : mode === 'debate' ? payload.topic : payload.prompt)
        .toBe('Keep this task through every view');
      if (mode !== 'solo') {
        expect(mode === 'fusion' ? payload.variants : mode === 'debate' ? payload.debaters : payload.workers).toHaveLength(3);
      }
    },
  );
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
    expect(codexLadder).toContain('gpt-6.1-sol');
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
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
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
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
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
