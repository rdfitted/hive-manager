import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import AgentConfigEditor from './AgentConfigEditor.svelte';

afterEach(() => cleanup());

describe('AgentConfigEditor', () => {
  it('recognizes the canonical model-only GPT-5.6 Sol default', () => {
    const { getByLabelText } = render(AgentConfigEditor, {
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
    expect(preset.value).toBe('codex-gpt-5-6-sol');
    expect(preset.selectedOptions[0]?.textContent).toBe('GPT-5.6 Sol');
  });

  it('recognizes the legacy GPT-5.6 alias and shows the canonical Sol model', () => {
    const { container, getByLabelText, getByText } = render(AgentConfigEditor, {
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

    expect((getByLabelText('Model & Effort') as HTMLSelectElement).value).toBe('codex-gpt-5-6-sol-medium');
    expect(getByText('Effective: gpt-5.6-sol · medium effort')).toBeTruthy();
    expect(container.querySelector('#principal-one-cli')).toBeTruthy();
    expect(container.querySelector('#principal-one-preset')).toBeTruthy();
    expect(container.querySelector('#principal-one-label')).toBeTruthy();
  });

  it.each(['max', 'ultra'] as const)('applies the GPT-5.6 Sol %s preset', async (effort) => {
    const { getByLabelText, getByText } = render(AgentConfigEditor, {
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
    preset.value = `codex-gpt-5-6-sol-${effort}`;
    await fireEvent.change(preset);

    expect(preset.value).toBe(`codex-gpt-5-6-sol-${effort}`);
    expect(getByText(`Effective: gpt-5.6-sol · ${effort} effort`)).toBeTruthy();
  });

  it('offers Fable 5 as a Claude preset and reflects max effort', () => {
    const { getByLabelText, getByText } = render(AgentConfigEditor, {
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
    expect(preset.value).toBe('fable-max');
    expect(Array.from(preset.options).some((option) => option.textContent === 'Fable 5 (Max effort)')).toBe(true);
    expect(getByText('Effective: fable · max effort')).toBeTruthy();
  });
});
