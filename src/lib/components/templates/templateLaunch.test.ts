import { describe, expect, it } from 'vitest';
import type { CellTemplate } from '$lib/types/domain';
import { routeFusionTemplateCells } from './templateLaunch';

function cell(role: string, cli: string, model?: string): CellTemplate {
  return {
    role,
    cli,
    model,
    prompt_template: role,
  };
}

describe('routeFusionTemplateCells', () => {
  it('routes resolver cells to judge config instead of launching them as candidates', () => {
    const result = routeFusionTemplateCells([
      cell('candidate-a', 'codex', 'gpt-5.6-sol'),
      cell('candidate-b', 'codex', 'gpt-5.6-terra'),
      cell('resolver', 'claude', 'opus'),
    ]);

    expect(result.variants).toEqual([
      { name: 'Variant A', cli: 'codex', model: 'gpt-5.6-sol', flags: [] },
      { name: 'Variant B', cli: 'codex', model: 'gpt-5.6-terra', flags: [] },
    ]);
    expect(result.judgeConfig).toEqual({
      cli: 'claude',
      model: 'opus',
      flags: [],
      label: 'Fusion Resolver',
    });
  });

  it('keeps every cell as a candidate when a template has no resolver or judge', () => {
    const result = routeFusionTemplateCells([
      cell('candidate-a', 'codex', 'gpt-5.6-sol'),
      cell('candidate-b', 'claude', 'fable'),
    ]);

    expect(result.variants).toHaveLength(2);
    expect(result.judgeConfig).toBeUndefined();
  });

  it('recognizes explicit judge aliases without leaking them into the variant list', () => {
    const result = routeFusionTemplateCells([
      cell('candidate-a', 'codex', 'gpt-5.6-sol'),
      cell('fusion_judge', 'claude', 'opus'),
    ]);

    expect(result.variants).toHaveLength(1);
    expect(result.judgeConfig?.label).toBe('Fusion Judge');
  });
});
