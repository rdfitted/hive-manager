import {
  CheckCircle,
  Hourglass,
  Lightning,
  NotePencil,
  Question,
  Rocket,
  Skull,
  Wrench,
  XCircle,
} from 'phosphor-svelte';
import type { CellStatus } from '$lib/types/domain';

const statusIcons: Partial<Record<CellStatus, any>> = {
  queued: Hourglass,
  preparing: Wrench,
  launching: Rocket,
  running: Lightning,
  summarizing: NotePencil,
  completed: CheckCircle,
  waiting_input: Question,
  failed: XCircle,
  killed: Skull,
};

export function statusIconFor(status: string): any {
  return statusIcons[status as CellStatus] ?? Question;
}

export function statusIconWeight(status: string): 'fill' | 'light' {
  return status === 'completed' || status === 'failed' ? 'fill' : 'light';
}
