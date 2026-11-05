export interface PinCandidate { id: string; width: number; queen: boolean; attention: boolean; }

/** Reserve a full scrolling column; place only the pins that fit, without overlap. */
export function columnPins(available: number, columns: PinCandidate[], minimum = 280, gap = 12): Record<string, number> {
  const pins: Record<string, number> = {};
  let left = 4;
  const priority = [...columns.filter(column => column.queen), ...columns.filter(column => !column.queen && column.attention)];
  for (const column of priority) {
    if (!Number.isFinite(column.width) || column.width <= 0) continue;
    if (left + column.width + gap + minimum + 4 > available) {
      if (column.queen) return pins;
      continue;
    }
    pins[column.id] = left;
    left += column.width + gap;
  }
  return pins;
}
