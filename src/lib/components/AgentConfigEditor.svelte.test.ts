import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { fetchPresetCatalogue } from './AgentConfigEditor.svelte';
import AgentConfigEditorHarness from './AgentConfigEditor.test-harness.svelte';

const presets = [
  { provider: 'claude', id: 'fable-high', label: 'Fable 5 (High effort)', model: 'fable', flags: ['--settings', '{"effortLevel":"high"}'] },
  { provider: 'claude', id: 'fable-max', label: 'Fable 5 (Max effort)', model: 'fable', flags: ['--settings', '{"effortLevel":"max"}'] },
  { provider: 'claude', id: 'fable', label: 'Fable 5', model: 'fable', flags: [] },
  { provider: 'codex', id: 'codex-gpt-5-6-sol', label: 'GPT-5.6 Sol', model: 'gpt-5.6-sol', flags: [] },
  { provider: 'codex', id: 'codex-gpt-5-6-sol-medium', label: 'GPT-5.6 Sol (Medium effort)', model: 'gpt-5.6-sol', flags: ['-c', 'model_reasoning_effort="medium"'] },
  { provider: 'codex', id: 'codex-gpt-5-6-sol-high', label: 'GPT-5.6 Sol (High effort)', model: 'gpt-5.6-sol', flags: ['-c', 'model_reasoning_effort="high"'] },
  { provider: 'codex', id: 'codex-gpt-5-6-sol-max', label: 'GPT-5.6 Sol (Max effort)', model: 'gpt-5.6-sol', flags: ['-c', 'model_reasoning_effort="max"'] },
  { provider: 'codex', id: 'codex-gpt-5-6-sol-ultra', label: 'GPT-5.6 Sol (Ultra effort)', model: 'gpt-5.6-sol', flags: ['-c', 'model_reasoning_effort="ultra"'] },
  { provider: 'cursor', id: 'composer-2.5', label: 'Composer 2.5 (latest)', model: 'composer-2.5', flags: [] },
  { provider: 'droid', id: 'glm-5.1', label: 'GLM 5.1', model: 'glm-5.1', flags: [] },
  { provider: 'opencode', id: 'opencode/big-pickle', label: 'BigPickle', model: 'opencode/big-pickle', flags: [] },
  { provider: 'qwen', id: 'qwen3-coder', label: 'Qwen3 Coder', model: 'qwen3-coder', flags: [] },
];

function jsonResponse(payload: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: vi.fn().mockResolvedValue(payload),
  } as unknown as Response;
}

beforeEach(async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({ presets })));
  await fetchPresetCatalogue(true);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('AgentConfigEditor', () => {
  it('recognizes the canonical model-only GPT-5.6 Sol default', async () => {
    const { getByLabelText } = render(AgentConfigEditorHarness, {
      props: {
        idPrefix: 'principal-default',
        config: {
          cli: 'codex',
          model: 'gpt-5.6-sol',
          flags: [],
        },
      },
    });

    const preset = getByLabelText('Model & Effort') as HTMLSelectElement;
    await waitFor(() => expect(preset.value).toBe('codex-gpt-5-6-sol'));
    expect(preset.selectedOptions[0]?.textContent).toBe('GPT-5.6 Sol');
  });

  it('recognizes the legacy GPT-5.6 alias and shows the canonical Sol model', async () => {
    const { container, getByLabelText, getByText } = render(AgentConfigEditorHarness, {
      props: {
        idPrefix: 'principal-one',
        showLabel: true,
        config: {
          cli: 'codex',
          model: 'gpt-5.6',
          flags: ['-c', 'model_reasoning_effort="medium"'],
          label: 'Coding Principal 1',
        },
      },
    });

    const preset = getByLabelText('Model & Effort') as HTMLSelectElement;
    await waitFor(() => expect(preset.value).toBe('codex-gpt-5-6-sol-medium'));
    expect(getByText('Effective: gpt-5.6-sol · medium effort')).toBeTruthy();
    expect(container.querySelector('#principal-one-cli')).toBeTruthy();
    expect(container.querySelector('#principal-one-preset')).toBeTruthy();
    expect(container.querySelector('#principal-one-label')).toBeTruthy();
  });

  it.each(['max', 'ultra'] as const)('applies the GPT-5.6 Sol %s preset', async (effort) => {
    const { getByLabelText, getByText } = render(AgentConfigEditorHarness, {
      props: {
        idPrefix: `principal-${effort}`,
        config: {
          cli: 'codex',
          model: 'gpt-5.6-sol',
          flags: [],
        },
      },
    });

    const preset = getByLabelText('Model & Effort') as HTMLSelectElement;
    await waitFor(() => expect(preset.options.length).toBeGreaterThan(2));
    await fireEvent.change(preset, { target: { value: `codex-gpt-5-6-sol-${effort}` } });

    expect(preset.value).toBe(`codex-gpt-5-6-sol-${effort}`);
    expect(getByText(`Effective: gpt-5.6-sol · ${effort} effort`)).toBeTruthy();
  });

  it('offers Fable 5 as a Claude preset and reflects max effort', async () => {
    const { getByLabelText, getByText } = render(AgentConfigEditorHarness, {
      props: {
        idPrefix: 'queen',
        config: {
          cli: 'claude',
          model: 'fable',
          flags: ['--settings', JSON.stringify({ effortLevel: 'max' })],
        },
      },
    });

    const preset = getByLabelText('Model & Effort') as HTMLSelectElement;
    await waitFor(() => expect(preset.value).toBe('fable-max'));
    expect(Array.from(preset.options).some((option) => option.textContent === 'Fable 5 (Max effort)')).toBe(true);
    expect(getByText('Effective: fable · max effort')).toBeTruthy();
  });

  async function expectServerPresetRoundTrip({
    cli,
    presetId,
    model,
    flags,
  }: {
    cli: string;
    presetId: string;
    model: string;
    flags: readonly string[];
  }) {
    const { getByLabelText, getByTestId } = render(AgentConfigEditorHarness, {
      props: {
        idPrefix: `round-trip-${cli}`,
        config: {
          cli,
          model: 'custom-model',
          flags: [],
        },
      },
    });
    const preset = getByLabelText('Model & Effort') as HTMLSelectElement;
    await waitFor(() => expect(Array.from(preset.options).some((option) => option.value === presetId)).toBe(true));
    await fireEvent.change(preset, { target: { value: presetId } });

    expect(JSON.parse(getByTestId('config').textContent || 'null')).toEqual({
      cli,
      model,
      flags: [...flags],
    });
  }

  it('round-trips a Claude preset from the server catalogue', async () => {
    await expectServerPresetRoundTrip({
      cli: 'claude',
      presetId: 'fable-high',
      model: 'fable',
      flags: ['--settings', '{"effortLevel":"high"}'],
    });
  });

  it('round-trips a Codex preset from the server catalogue', async () => {
    await expectServerPresetRoundTrip({
      cli: 'codex',
      presetId: 'codex-gpt-5-6-sol-high',
      model: 'gpt-5.6-sol',
      flags: ['-c', 'model_reasoning_effort="high"'],
    });
  });

  it('round-trips a Cursor preset from the server catalogue', async () => {
    await expectServerPresetRoundTrip({
      cli: 'cursor',
      presetId: 'composer-2.5',
      model: 'composer-2.5',
      flags: [],
    });
  });

  it('round-trips a Droid preset from the server catalogue', async () => {
    await expectServerPresetRoundTrip({
      cli: 'droid',
      presetId: 'glm-5.1',
      model: 'glm-5.1',
      flags: [],
    });
  });

  it('round-trips an OpenCode preset from the server catalogue', async () => {
    await expectServerPresetRoundTrip({
      cli: 'opencode',
      presetId: 'opencode/big-pickle',
      model: 'opencode/big-pickle',
      flags: [],
    });
  });

  it('round-trips a Qwen preset from the server catalogue', async () => {
    await expectServerPresetRoundTrip({
      cli: 'qwen',
      presetId: 'qwen3-coder',
      model: 'qwen3-coder',
      flags: [],
    });
  });

  it('keeps the custom selection as an early return', async () => {
    const config = {
      cli: 'codex',
      model: 'gpt-5.6-sol',
      flags: ['-c', 'model_reasoning_effort="medium"'],
    };
    const { getByLabelText, getByTestId } = render(AgentConfigEditorHarness, {
      props: { idPrefix: 'custom', config },
    });

    const preset = getByLabelText('Model & Effort') as HTMLSelectElement;
    await waitFor(() => expect(preset.value).toBe('codex-gpt-5-6-sol-medium'));
    await fireEvent.change(preset, { target: { value: 'custom' } });

    expect(getByTestId('change-count').textContent).toBe('0');
    expect(config).toEqual({
      cli: 'codex',
      model: 'gpt-5.6-sol',
      flags: ['-c', 'model_reasoning_effort="medium"'],
    });
  });

  it('retains unmanaged flags while replacing managed effort flags', async () => {
    const { getByLabelText, getByTestId } = render(AgentConfigEditorHarness, {
      props: {
        idPrefix: 'unmanaged',
        config: {
          cli: 'codex',
          model: 'gpt-5.6-sol',
          flags: ['--sandbox', 'workspace-write', '-c', 'model_reasoning_effort="high"'],
        },
      },
    });
    const preset = getByLabelText('Model & Effort') as HTMLSelectElement;
    await waitFor(() => expect(Array.from(preset.options).some(
      (option) => option.value === 'codex-gpt-5-6-sol-medium',
    )).toBe(true));
    await fireEvent.change(preset, {
      target: { value: 'codex-gpt-5-6-sol-medium' },
    });

    expect(JSON.parse(getByTestId('config').textContent || 'null')).toEqual({
      cli: 'codex',
      model: 'gpt-5.6-sol',
      flags: ['--sandbox', 'workspace-write', '-c', 'model_reasoning_effort="medium"'],
    });
  });
});
