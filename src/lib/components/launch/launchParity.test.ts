// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import LaunchDialog from '../LaunchDialog.svelte';
import { selectedTemplate } from '$lib/stores/templates';
import type { SessionTemplate } from '$lib/types/domain';
import { builtinFixtures, legacyPayload } from './launchFixtures';

const mocks = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: mocks.open }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

const project = 'D:/code/parity project';
const task = '  Implement the first step\nThen verify the result.  ';
const modes = ['hive', 'research', 'solo', 'fusion', 'debate'] as const;
type Mode = typeof modes[number];

beforeEach(() => {
  mocks.open.mockReset().mockResolvedValue(project);
  selectedTemplate.set(null);
  vi.stubGlobal('fetch', vi.fn(async (url: string) => new Response(JSON.stringify(
    url.includes('/api/templates') ? { templates: builtinFixtures, role_packs: [] }
      : url.includes('/api/preset-catalogue') ? { presets: [{ provider: 'codex', id: 'high', label: 'High', model: 'gpt-6.1-sol', flags: ['-c', 'model_reasoning_effort="xhigh"'] }] }
      : url.includes('/api/tier-ladder') ? { cells: [], omissions: [] } : { clis: [] },
  ), { status: 200 })));
});
afterEach(() => { cleanup(); selectedTemplate.set(null); vi.unstubAllGlobals(); });

function openDialog(mode: Mode | 'templates' = 'hive') {
  const launch = vi.fn();
  const view = render(LaunchDialog, {
    props: { show: true, initialMode: mode },
    events: { launchHive: launch, launchResearch: launch, launchSolo: launch, launchFusion: launch, launchDebate: launch },
  });
  return { ...view, launch };
}

async function editAdvancedTask(view: ReturnType<typeof openDialog>, mode: Mode, text: string) {
  if (mode === 'solo') await fireEvent.input(view.getByLabelText('Task Description'), { target: { value: text } });
  else {
    const editor = view.container.querySelector<HTMLElement>('[data-composer]')!;
    editor.textContent = text;
    await fireEvent.input(editor);
  }
}

async function capture(mode: Mode, advanced: boolean, template?: SessionTemplate) {
  const view = openDialog(mode);
  if (advanced) {
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
    if (template) { selectedTemplate.set(template); await tick(); }
    await fireEvent.click(view.getByRole('button', { name: 'Browse' }));
    await editAdvancedTask(view, mode, task);
  } else {
    if (template) {
      await waitFor(() => expect(view.getByRole('option', { name: template.name })).toBeTruthy());
      await fireEvent.change(view.getByLabelText('Preset'), { target: { value: template.id } });
    }
    await fireEvent.click(view.getByRole('button', { name: /Project/ }));
    await fireEvent.input(view.getByLabelText('What would you like to work on?'), { target: { value: task } });
  }
  await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
  expect(view.launch).toHaveBeenCalledTimes(1);
  const emitted = view.launch.mock.calls[0][0] as CustomEvent;
  const result = { event: emitted.type, payload: structuredClone(emitted.detail) };
  view.unmount();
  return result;
}

describe('G-launch-parity: emitted composer and retained legacy-form payloads', () => {
  it.each(modes)('%s matches the legacy event and independent default oracle', async (mode) => {
    const oldForm = await capture(mode, true);
    const composer = await capture(mode, false);
    expect(composer).toStrictEqual(oldForm);
    expect(composer).toStrictEqual({ event: `launch${mode.charAt(0).toUpperCase()}${mode.slice(1)}`, payload: legacyPayload(mode, project, task) });
  });

  it.each(builtinFixtures)('$id preserves the complete canonical template payload', async (template) => {
    const mode = template.mode as Mode;
    const oldForm = await capture(mode, true, template);
    const composer = await capture(mode, false, template);
    expect(composer).toStrictEqual(oldForm);
    expect(composer).toStrictEqual({ event: `launch${mode.charAt(0).toUpperCase()}${mode.slice(1)}`, payload: legacyPayload(mode, project, task, template) });
  });

  it('templates is a non-launching state on both surfaces', async () => {
    for (const advanced of [false, true]) {
      const view = openDialog('templates');
      if (advanced) await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
      await fireEvent.click(view.getByRole('button', { name: advanced ? 'Browse' : /Project/ }));
      expect((view.getByRole('button', { name: 'Launch' }) as HTMLButtonElement).disabled).toBe(true);
      await fireEvent.submit(view.container.querySelector('form')!);
      expect(view.launch).not.toHaveBeenCalled();
      expect(view.getByRole('alert').textContent).toContain('Choose a template');
      view.unmount();
    }
  });

  it.each(['hive', 'research', 'fusion', 'debate'] as const)('%s maps the worker chip into the retained form roster', async (mode) => {
    const view = openDialog(mode);
    await fireEvent.click(view.getByRole('button', { name: /Project/ }));
    await fireEvent.input(view.getByLabelText('What would you like to work on?'), { target: { value: task } });
    await fireEvent.change(view.getByLabelText('Workers'), { target: { value: '3' } });
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    expect(view.launch.mock.calls[0][0].detail).toStrictEqual(legacyPayload(mode, project, task, undefined, 3));
  });

  it.each(modes)('Mode chip routes %s to its real event and complete legacy payload', async (mode) => {
    const view = openDialog();
    await fireEvent.input(view.getByLabelText('What would you like to work on?'), { target: { value: task } });
    await fireEvent.change(view.getByLabelText('Mode'), { target: { value: mode } });
    await fireEvent.click(view.getByRole('button', { name: /Project/ }));
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    expect(view.launch).toHaveBeenCalledTimes(1);
    const emitted = view.launch.mock.calls[0][0] as CustomEvent;
    expect(emitted.type).toBe(`launch${mode.charAt(0).toUpperCase()}${mode.slice(1)}`);
    expect(emitted.detail).toStrictEqual(legacyPayload(mode, project, task));
  });

  it.each(builtinFixtures)('$id fills an empty task and chips, preserving an existing task', async (template) => {
    const view = openDialog();
    await waitFor(() => expect(view.getByRole('option', { name: template.name })).toBeTruthy());
    await fireEvent.change(view.getByLabelText('Preset'), { target: { value: template.id } });
    expect((view.getByLabelText('What would you like to work on?') as HTMLTextAreaElement).value).toBe(template.description);
    expect((view.getByLabelText('Mode') as HTMLSelectElement).value).toBe(template.mode);
    expect((view.getByLabelText('Workers') as HTMLSelectElement).value).toBe(String(template.id === 'feature-build-hive' ? 3 : 2));
    await fireEvent.input(view.getByLabelText('What would you like to work on?'), { target: { value: task } });
    await fireEvent.change(view.getByLabelText('Preset'), { target: { value: template.id } });
    expect((view.getByLabelText('What would you like to work on?') as HTMLTextAreaElement).value).toBe(task);
  });

  it.each(builtinFixtures)('Solo task survives selecting $id and is the task actually launched', async (template) => {
    const view = openDialog('solo');
    await fireEvent.input(view.getByLabelText('What would you like to work on?'), { target: { value: task } });
    await waitFor(() => expect(view.getByRole('option', { name: template.name })).toBeTruthy());
    await fireEvent.change(view.getByLabelText('Preset'), { target: { value: template.id } });
    expect((view.getByLabelText('What would you like to work on?') as HTMLTextAreaElement).value).toBe(task);
    await fireEvent.click(view.getByRole('button', { name: /Project/ }));
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    expect(view.launch.mock.calls[0][0].detail).toStrictEqual(legacyPayload(template.mode, project, task, template));
  });

  it('keeps every Advanced edit when returning to the composer', async () => {
    const view = openDialog();
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
    await fireEvent.click(view.getByRole('button', { name: 'Browse' }));
    await fireEvent.input(view.getByLabelText('Session Name (optional)'), { target: { value: '  Named session  ' } });
    await fireEvent.click(view.getByTitle('Blue'));
    await fireEvent.click(view.getByLabelText('Per-principal worktrees', { exact: false }));
    await fireEvent.input(view.getByLabelText('Archetype'), { target: { value: ' audit ' } });
    await fireEvent.click(view.getByRole('button', { name: '+ Add override' }));
    await fireEvent.input(view.getByLabelText('Parameter name'), { target: { value: ' component ' } });
    await fireEvent.input(view.getByLabelText('Parameter value'), { target: { value: ' launch ' } });
    await fireEvent.change(view.getByLabelText('Queen delegation'), { target: { value: 'disabled' } });
    await fireEvent.click(view.getByRole('checkbox', { name: /Enable tier-based model routing/ }));
    await fireEvent.input(view.getByLabelText('Routing ceiling (%)'), { target: { value: '72' } });
    await fireEvent.click(view.getByRole('checkbox', { name: /QA: Evaluator \+ QA workers/ }));
    await editAdvancedTask(view, 'hive', task);
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    const oldForm = structuredClone(view.launch.mock.calls[0][0].detail);
    await fireEvent.click(view.getByRole('button', { name: 'Back to composer' }));
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    const composer = view.launch.mock.calls[1][0].detail;
    expect(composer).toStrictEqual(oldForm);
    expect(composer).toMatchObject({ name: 'Named session', color: '#7aa2f7', work_graph_archetype: 'audit', work_graph_parameters: { component: 'launch' }, execution_policy: { workspace_strategy: 'isolated_cell', queen_delegation: { mode: 'disabled' }, tier_policy: { enabled: true, ceiling_percent: 72 } }, with_evaluator: false });
    expect(composer.evaluator_config).toBeUndefined();
    expect(composer.qa_workers).toBeUndefined();
  });

  it.each([
    ['hive', 'launch-principal-0', 'workers'],
    ['research', 'launch-researcher-0', 'workers'],
    ['solo', 'launch-solo', null],
    ['fusion', 'launch-fusion-0', 'variants'],
    ['debate', 'launch-debater-0', 'debaters'],
  ] as const)('%s retains edited per-agent CLI, model and effort flags through the composer', async (mode, prefix, roster) => {
    const view = openDialog(mode);
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
    await fireEvent.click(view.getByRole('button', { name: 'Browse' }));
    await fireEvent.change(view.container.querySelector(`#${prefix}-cli`)!, { target: { value: 'codex' } });
    await waitFor(() => expect(view.container.querySelector(`#${prefix}-preset option[value="high"]`)).toBeTruthy());
    await fireEvent.change(view.container.querySelector(`#${prefix}-preset`)!, { target: { value: 'high' } });
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    const oldForm = structuredClone(view.launch.mock.calls[0][0].detail);
    expect(roster ? oldForm[roster][0] : oldForm).toMatchObject({ cli: 'codex', model: 'gpt-6.1-sol', flags: ['-c', 'model_reasoning_effort="xhigh"'] });
    await fireEvent.click(view.getByRole('button', { name: 'Back to composer' }));
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    expect(view.launch.mock.calls[1][0].detail).toStrictEqual(oldForm);
  });
});

describe('composer input contract', () => {
  it('opens with current role defaults and the Custom preset, keeping the full form behind Advanced', async () => {
    const view = openDialog();
    expect((view.getByLabelText('Preset') as HTMLSelectElement).value).toBe('custom');
    expect(view.queryByLabelText('Archetype')).toBeNull();
    await fireEvent.click(view.getByRole('button', { name: /Project/ }));
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
    expect(view.getByLabelText('Archetype')).toBeTruthy();
    await waitFor(() => expect((view.container.querySelector('#launch-principal-0-preset') as HTMLSelectElement).value).toBe('custom'));
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    expect(view.launch.mock.calls[0][0].detail).toStrictEqual(legacyPayload('hive', project, ''));
  });

  it('plain Enter is left to the textarea as a newline and never launches', async () => {
    const view = openDialog();
    await fireEvent.click(view.getByRole('button', { name: /Project/ }));
    const editor = view.getByLabelText('What would you like to work on?');
    const event = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
    editor.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    await fireEvent.input(editor, { target: { value: 'First line\nSecond line' } });
    expect(view.launch).not.toHaveBeenCalled();
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    expect(view.launch.mock.calls[0][0].detail.prompt).toBe('First line\nSecond line');
  });

  it.each(['ctrlKey', 'metaKey'] as const)('%s+Enter launches the populated prompt once', async (modifier) => {
    const view = openDialog();
    await fireEvent.click(view.getByRole('button', { name: /Project/ }));
    const editor = view.getByLabelText('What would you like to work on?');
    await fireEvent.input(editor, { target: { value: task } });
    await fireEvent.keyDown(editor, { key: 'Enter', [modifier]: true });
    expect(view.launch).toHaveBeenCalledTimes(1);
    expect(view.launch.mock.calls[0][0].detail.prompt).toBe(task);
  });

  it('mode changes carry task text through Solo without changing its payload shape', async () => {
    const view = openDialog();
    await fireEvent.input(view.getByLabelText('What would you like to work on?'), { target: { value: task } });
    await fireEvent.change(view.getByLabelText('Mode'), { target: { value: 'solo' } });
    expect((view.getByLabelText('What would you like to work on?') as HTMLTextAreaElement).value).toBe(task);
    await fireEvent.change(view.getByLabelText('Mode'), { target: { value: 'debate' } });
    expect((view.getByLabelText('What would you like to work on?') as HTMLTextAreaElement).value).toBe(task);
  });

  it('launching disables button and keyboard submission and shows launch errors', async () => {
    const view = openDialog();
    await fireEvent.click(view.getByRole('button', { name: /Project/ }));
    await view.rerender({ show: true, launching: true, launchError: 'Could not start session' });
    await fireEvent.keyDown(view.getByLabelText('What would you like to work on?'), { key: 'Enter', ctrlKey: true });
    expect((view.getByRole('button', { name: 'Launching...' }) as HTMLButtonElement).disabled).toBe(true);
    expect(view.getByRole('alert').textContent).toBe('Could not start session');
    expect(view.launch).not.toHaveBeenCalled();
  });
});

describe('G-qa-parity: composer controls reach emitted payloads', () => {
  async function captureQa(mode: 'hive' | 'solo', advanced: boolean, enabled: boolean) {
    const view = openDialog(mode);
    if (advanced) {
      await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
      await fireEvent.click(view.getByRole('button', { name: 'Browse' }));
      await editAdvancedTask(view, mode, task);
      if (!enabled) await fireEvent.click(view.getByRole('checkbox', { name: /QA: Evaluator \+ QA workers/ }));
    } else {
      await fireEvent.click(view.getByRole('button', { name: /Project/ }));
      await fireEvent.input(view.getByLabelText('What would you like to work on?'), { target: { value: task } });
      if (!enabled) await fireEvent.click(view.getByRole('button', { name: /^QA: on/ }));
    }
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));
    expect(view.launch).toHaveBeenCalledOnce();
    const payload = structuredClone(view.launch.mock.calls[0][0].detail);
    view.unmount();
    return payload;
  }

  it.each(['hive', 'solo'] as const)('%s QA-off matches Advanced with no evaluator config or QA roster', async (mode) => {
    const composer = await captureQa(mode, false, false);
    expect(composer).toStrictEqual(await captureQa(mode, true, false));
    expect(composer.with_evaluator).toBe(false);
    expect(composer.evaluator_config).toBeUndefined();
    expect(composer.qa_workers).toBeUndefined();
  });
  it.each(['hive', 'solo'] as const)('%s QA-on retains the independent default payload', async (mode) => {
    const composer = await captureQa(mode, false, true);
    expect(composer).toStrictEqual(await captureQa(mode, true, true));
    expect(composer).toStrictEqual(legacyPayload(mode, project, task));
  });
  it.each(['hive', 'solo'] as const)('%s QA toggles round-trip through Advanced and the live roster summary', async (mode) => {
    const view = openDialog(mode);
    expect(view.getByRole('button', { name: 'QA: on · Evaluator + 3 (UI, API, A11Y)' }).getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(view.getByRole('button', { name: /^QA: on/ }));
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
    const qa = view.getByRole('checkbox', { name: /QA: Evaluator \+ QA workers/ });
    expect((qa as HTMLInputElement).checked).toBe(false);
    await fireEvent.click(qa);
    await fireEvent.click(view.getByRole('button', { name: '+ Add' }));
    await fireEvent.click(view.getByRole('button', { name: 'Back to composer' }));
    expect(view.getByRole('button', { name: 'QA: on · Evaluator + 4 (UI, API, A11Y, UI)' }).getAttribute('aria-pressed')).toBe('true');
  });
  it.each(['hive', 'research'] as const)('%s Smoke Test matches Advanced and changes only smoke_test', async (mode) => {
    const payloads = [];
    for (const advanced of [false, true]) {
      const view = openDialog(mode);
      if (advanced) {
        await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
        await fireEvent.click(view.getByRole('button', { name: 'Browse' }));
        await editAdvancedTask(view, mode, task);
      } else {
        await fireEvent.click(view.getByRole('button', { name: /Project/ }));
        await fireEvent.input(view.getByLabelText('What would you like to work on?'), { target: { value: task } });
      }
      await fireEvent.click(view.getByRole('button', { name: 'Smoke Test' }));
      expect(view.launch).toHaveBeenCalledOnce();
      payloads.push(structuredClone(view.launch.mock.calls[0][0].detail));
      view.unmount();
    }
    expect(payloads[0]).toStrictEqual(payloads[1]);
    expect(payloads[0]).toStrictEqual({ ...legacyPayload(mode, project, task), smoke_test: true });
  });
  it.each(['fusion', 'debate', 'research'] as const)('%s remains byte-identical to its independent baseline payload', async (mode) => {
    expect(JSON.stringify((await capture(mode, false)).payload)).toBe(JSON.stringify(legacyPayload(mode, project, task)));
  });
});

describe('dialog composer reachability by mode', () => {
  it.each(modes)('%s exposes only the supported quick controls', (mode) => {
    const view = openDialog(mode);
    expect(!!view.queryByRole('button', { name: /^QA:/ })).toBe(mode === 'hive' || mode === 'solo');
    expect(!!view.queryByRole('button', { name: 'Smoke Test' })).toBe(mode === 'hive' || mode === 'research');
  });
});

describe('T6 Cancel presentation contract', () => {
  it('Cancel is absent inline and present in the modal on the same instance', async () => {
    const view = render(LaunchDialog, { props: { show: true, presentation: 'inline' } });
    expect(view.queryByRole('button', { name: 'Cancel' })).toBeNull();
    await view.rerender({ presentation: 'modal' });
    expect(view.getByRole('button', { name: 'Cancel' })).toBeTruthy();
    await view.rerender({ presentation: 'inline' });
    expect(view.queryByRole('button', { name: 'Cancel' })).toBeNull();
  });
});
