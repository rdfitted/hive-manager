/**
 * Built-in renderer registration.
 *
 * This is the single place the four built-in widgets are wired to their ids.
 * `registerBuiltinRenderers()` is idempotent (guarded by a module flag) and is
 * called once from ToolRenderHost on init.
 */

import { registerToolRenderer, type ToolRendererComponent } from './registry';
import DataTable from './DataTable.svelte';
import Diff from './Diff.svelte';
import Approval from './Approval.svelte';
import Chart from './Chart.svelte';

let registered = false;

export function registerBuiltinRenderers(): void {
  if (registered) {
    return;
  }
  registered = true;

  registerToolRenderer({ id: 'table', component: DataTable as ToolRendererComponent });
  registerToolRenderer({ id: 'diff', component: Diff as ToolRendererComponent });
  registerToolRenderer({ id: 'approval', component: Approval as ToolRendererComponent });
  registerToolRenderer({ id: 'chart', component: Chart as ToolRendererComponent });
}

/**
 * Test helper: reset the idempotency guard so a subsequent
 * registerBuiltinRenderers() re-runs (used after clearAllRenderers()).
 */
export function resetBuiltinRegistration(): void {
  registered = false;
}
