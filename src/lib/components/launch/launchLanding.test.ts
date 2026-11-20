// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import LaunchDialog from '../LaunchDialog.svelte';
import { selectedTemplate } from '$lib/stores/templates';
import { builtinFixtures, legacyPayload } from './launchFixtures';

const project = 'D:/code/landing project';
const task = '  Implement the first step\nThen verify the result.  ';
const taskLabel = 'What would you like to work on?';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

beforeEach(() => {
  selectedTemplate.set(null);
  vi.stubGlobal('fetch', vi.fn(async (url: string) => new Response(JSON.stringify(
    url.includes('/api/templates') ? { templates: builtinFixtures, role_packs: [] }
      : url.includes('/api/tier-ladder') ? { cells: [], omissions: [] } : { clis: [] },
  ), { status: 200 })));
});

afterEach(() => {
  cleanup();
  selectedTemplate.set(null);
  vi.unstubAllGlobals();
});

function renderLanding() {
  const launch = vi.fn<(event: CustomEvent) => void>();
  return {
    ...render(LaunchDialog, {
      props: { show: true, presentation: 'inline', defaultProjectPath: project },
      events: { launchHive: launch, launchSolo: launch, launchResearch: launch, launchFusion: launch, launchDebate: launch },
    }),
    launch,
  };
}

describe('launch landing', () => {
  it.each([
    ['Hive', 'hive'],
    ['Fusion', 'fusion'],
    ['Solo', 'solo'],
    ['Research', 'research'],
    ['Debate', 'debate'],
  ])('emits the unchanged %s launch event and payload', async (name, mode) => {
    const view = renderLanding();
    const modePill = view.getByRole('radio', { name }) as HTMLInputElement;
    await fireEvent.click(modePill);
    expect(modePill.checked).toBe(true);
    await fireEvent.input(view.getByLabelText(taskLabel), { target: { value: task } });
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));

    expect(view.launch).toHaveBeenCalledOnce();
    const event = view.launch.mock.calls[0][0];
    const expected = legacyPayload(mode, project, task);
    expect(event.type).toBe(`launch${name}`);
    expect(structuredClone(event.detail)).toStrictEqual(expected);
    expect(JSON.stringify(event.detail)).toBe(JSON.stringify(expected));
  });

  it('selects a preset from Templates and emits its base payload', async () => {
    const view = renderLanding();
    await fireEvent.click(view.getByRole('radio', { name: 'Templates' }));
    expect((view.getByRole('button', { name: 'Launch' }) as HTMLButtonElement).disabled).toBe(true);

    const template = builtinFixtures[0];
    await view.findByRole('option', { name: template.name });
    await fireEvent.change(view.getByLabelText('Preset'), { target: { value: template.id } });
    expect((view.getByRole('radio', { name: 'Hive' }) as HTMLInputElement).checked).toBe(true);
    await fireEvent.input(view.getByLabelText(taskLabel), { target: { value: task } });
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));

    expect(view.launch).toHaveBeenCalledOnce();
    expect(structuredClone(view.launch.mock.calls[0][0].detail)).toStrictEqual(legacyPayload('hive', project, task, template));
  });

  it.each(['Hive', 'Solo'])('keeps %s transport settings across Advanced and mode changes', async (name) => {
    const view = renderLanding();
    await fireEvent.click(view.getByRole('radio', { name }));
    const advanced = () => view.getByRole('button', { name: 'Advanced' });
    const transport = (label: string) => within(view.getByRole('group', { name: 'Agent transport' })).getByLabelText<HTMLSelectElement>(label);
    const labels = ['Codex transport', 'Claude transport'];

    for (const label of labels) expect(view.queryByLabelText(label)).toBeNull();
    await fireEvent.click(advanced());
    for (const label of labels) {
      expect(transport(label).value).toBe('pty');
      await fireEvent.change(transport(label), { target: { value: 'protocol' } });
    }
    await fireEvent.click(view.getByRole('button', { name: 'Back to composer' }));
    for (const label of labels) expect(view.queryByLabelText(label)).toBeNull();
    await fireEvent.click(view.getByRole('radio', { name: 'Research' }));
    await fireEvent.click(view.getByRole('radio', { name }));
    await fireEvent.click(advanced());
    for (const label of labels) expect(transport(label).value).toBe('protocol');
    await fireEvent.click(view.getByRole('button', { name: 'Back to composer' }));
    await fireEvent.click(view.getByRole('button', { name: 'Launch' }));

    expect(view.launch).toHaveBeenCalledOnce();
    expect(view.launch.mock.calls[0][0].detail.agent_transports).toEqual(['codex_app_server', 'claude_stream_json']);
  });

  it('leads with the mark and wordmark and focuses the task on arrival and return from Advanced', async () => {
    const view = renderLanding();
    const heading = view.getByRole('heading', { name: 'Hive Manager' });
    expect(heading.previousElementSibling?.querySelector('svg')).toBeTruthy();
    await waitFor(() => expect(document.activeElement).toBe(view.getByLabelText(taskLabel)));
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
    await fireEvent.click(view.getByRole('button', { name: 'Back to composer' }));
    await waitFor(() => expect(document.activeElement).toBe(view.getByLabelText(taskLabel)));
  });
});
