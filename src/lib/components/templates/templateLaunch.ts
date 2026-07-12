import type { AgentConfig, FusionVariantConfig } from '$lib/stores/sessions';
import type { CellTemplate } from '$lib/types/domain';

export interface FusionTemplateRouting {
  variants: FusionVariantConfig[];
  judgeConfig?: AgentConfig;
}

function normalizedRole(role: string): string {
  return role.trim().toLowerCase().replaceAll('_', '-');
}

function isFusionJudgeCell(cell: CellTemplate): boolean {
  const role = normalizedRole(cell.role);
  return role === 'judge'
    || role === 'resolver'
    || role.endsWith('-judge')
    || role.endsWith('-resolver');
}

/**
 * Session templates store Fusion candidates and the resolver in one cell list.
 * The launch command expects them on separate `variants` and `judge_config`
 * fields, so split them deterministically before populating the launch form.
 */
export function routeFusionTemplateCells(cells: CellTemplate[]): FusionTemplateRouting {
  const candidateCells = cells.filter((cell) => !isFusionJudgeCell(cell));
  const judgeCell = cells.find(isFusionJudgeCell);

  return {
    variants: candidateCells.map((cell, index) => ({
      name: `Variant ${String.fromCharCode(65 + index)}`,
      cli: cell.cli,
      model: cell.model,
      flags: [],
    })),
    judgeConfig: judgeCell
      ? {
          cli: judgeCell.cli,
          model: judgeCell.model,
          flags: [],
          label: normalizedRole(judgeCell.role).includes('resolver')
            ? 'Fusion Resolver'
            : 'Fusion Judge',
        }
      : undefined,
  };
}
